//! Place validation, subscript/index inference and assignment (`__getitem__`/
//! `__setitem__`, slices, multi-index), pointer offset/write checks, and member
//! access inference. Extracted from `checker.rs`; see `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

/// A nominal subscript as an assignment target, as upstream's
/// `emitGetterSetterAccess` forms it.
pub(super) enum SubscriptLValue {
    /// Only a getter: its result is the place, written through when it is a
    /// mutable reference.
    Getter,
    /// A getter and a setter (upstream's `SubscriptDLValue`).
    GetterSetter {
        /// The selected getter's result, a `ref` result looked through.
        element: Ty,
        /// Whether that getter returns a reference.
        getter_returns_ref: bool,
        /// The keyword a store passes its value under: the last argument
        /// of the first declared `__setitem__`.
        value_name: String,
    },
}

impl Checker {
    /// Validate an assignment **place** and return the type stored there. A place
    /// is a chain of field (`.x`) and index (`[i]`) accesses over a root that
    /// must be a mutable location: any variable, or `self` in a `mut self`
    /// method. Recursing on the object of each step verifies the whole chain is
    /// rooted at a mutable place (so `foo().x = e` or `self.x` in a read-only
    /// method are rejected). SIMD lane writes are not supported yet.
    pub(super) fn check_place(&self, place: &Expr) -> Result<Ty, TypeError> {
        let result = self.check_place_impl(place).and_then(|ty| {
            self.check_rebind_place(place, &ty)?;
            self.apply_rebind_target(place, ty)
        });
        if let Ok(ty) = &result {
            self.record_at(place.source_span()).ty = Some(ty.clone());
            if let ExprKind::Identifier(name) = &place.kind
                && let Some(owner) = self.lookup_owner(name)
            {
                self.record_at(place.source_span()).binding = Some(owner);
            }
            let storage_ty = self.place_storage_ty(place).or_else(|| Some(ty.clone()));
            if let Some(storage_ty) = storage_ty {
                self.record_at(place.source_span()).place_ty = Some(storage_ty);
            }
        }
        result
    }

    pub(super) fn place_storage_ty(&self, place: &Expr) -> Option<Ty> {
        match &place.kind {
            ExprKind::Identifier(name) => self.lookup(name).cloned(),
            // A value pack's member (`vs.values`) reads no storage.
            ExprKind::Member { object, .. } if self.value_pack_named(object).is_some() => None,
            ExprKind::Member { object, field } => self.infer(object).ok().and_then(|base| {
                let TyKind::Struct(name, arguments) = base.kind() else {
                    return None;
                };
                let info = self.structs.get(name)?;
                let (_, field_ty) = info
                    .fields
                    .iter()
                    .find(|(candidate, _)| candidate == field)?;
                Some(substitute_at(field_ty, info, arguments))
            }),
            ExprKind::Index { object, index } => self.index_storage_ty(object, index),
            ExprKind::MultiIndex { .. } => {
                let (object, offset) = super::places::pointer_offset_keyword_subscript(place)?;
                self.index_storage_ty(object, offset)
            }
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
        match object_ty.kind() {
            TyKind::Tuple(elements) => {
                let index = usize::try_from(self.eval_ct(index).ok()?.to_i64()?).ok()?;
                elements.get(index).cloned()
            }
            TyKind::Pointer { element, .. } => Some((*element).clone()),
            TyKind::Simd { dtype, .. } => Some(simd_lane(dtype)),
            _ => None,
        }
    }

    #[allow(clippy::too_many_lines, reason = "TODO: split this pass")]
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
                    .map(|ty| match ty.kind() {
                        TyKind::Ref(reference) => reference.referent.clone(),
                        _ => ty.clone(),
                    })
                    .ok_or_else(|| TypeError::UndefinedVariable(name.clone()))
            }
            ExprKind::Member { object, field } => {
                // The object must itself be a writable place (a struct value).
                self.check_place(object)?;
                // Reuse the field-typing logic (validates the field exists).
                self.infer_member(place.source_span(), object, field)
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
                    .node_facts
                    .borrow()
                    .get(&place.source_span())
                    .and_then(|facts| facts.selected_call.as_deref())
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
                        self.record_at(place.source_span()).reference_value_use = Some(true);
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
                // The empty-subscript store `p[] = v` writes the pointee at
                // offset 0. The pointer itself is only read — a plain `self`
                // method may write through `self.p[]` — so the provenance,
                // not the pointer's own place, must carry mutable capability
                // (as upstream: a `Pointer[T, MutOrigin]` field is writable
                // from any receiver).
                if matches!(index.kind, ExprKind::EmptySubscript) {
                    return match (self.infer(object)?).kind() {
                        TyKind::Pointer { element, origin } => {
                            self.check_pointer_write(object, origin)?;
                            Ok((*element).clone())
                        }
                        other => Err(TypeError::Unsupported(format!(
                            "an empty subscript ('value[]') is the pointer dereference; \
                             '{other}' is not a pointer"
                        ))),
                    };
                }
                // A pointer is a value: a store through it needs its origin's
                // mutable capability (`check_pointer_write` below), not a
                // mutable binding, so `def w(p: Pointer[Int]): p[0] = 2`
                // writes through the parameter as upstream does.
                let inferred = self.infer(object)?;
                let obj_ty = match inferred.kind() {
                    TyKind::Pointer { .. } => inferred,
                    _ => self.check_place(object)?,
                };
                // A subscript-shaped Variant projection target (`storage[T] = x`
                // on a Variant-typed field, `v[String] = x`).
                if let TyKind::Variant(alternatives) = obj_ty.kind()
                    && let Some(arg) = self.variant_projection_type_argument(index)
                {
                    let alternatives = alternatives.clone();
                    let (index, alternative) =
                        self.variant_alternative(&alternatives, std::slice::from_ref(&arg))?;
                    self.record_at(place.source_span()).adjustment = Some(Box::new(
                        mojito_checked::checked::SemanticAdjustment::VariantProject {
                            alternatives,
                            index,
                        },
                    ));
                    if let ExprKind::Identifier(name) = &object.kind
                        && let Some(owner) = self.lookup_owner(name)
                    {
                        self.record_at(place.source_span()).binding = Some(owner);
                    }
                    return Ok(alternative);
                }
                // A declared struct's subscript is upstream's getter/setter
                // lvalue (`emitGetterSetterAccess`): as a place, it is
                // written through the getter's mutable reference.
                if let Some(lvalue) = self.subscript_lvalue(place)? {
                    return self.subscript_reference_place(place, &obj_ty, &lvalue);
                }
                self.prepare_index_argument(&obj_ty, index, "__setitem__", 0)?;
                if let TyKind::Struct(name, _) = obj_ty.kind()
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
                        return Ok(match element.clone().kind() {
                            TyKind::Ref(reference) => reference.referent.clone(),
                            _ => element.clone(),
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
                        return Ok(match value.clone().kind() {
                            TyKind::Ref(reference) => reference.referent.clone(),
                            _ => value.clone(),
                        });
                    }
                    if tuple_elements(&obj_ty).is_some() {
                        return Err(TypeError::InvalidAssignTarget(
                            "Tuple elements are immutable".to_string(),
                        ));
                    }
                }
                // Compiler-private heterogeneous storage is built or replaced
                // one element at a time: each compile-time index is a place
                // of its own inside the writable storage.
                if matches!(obj_ty.kind(), TyKind::Tuple(_)) {
                    self.check_place(object)?;
                    return self.infer(place);
                }
                let elem = match obj_ty.kind() {
                    // A pointer store `ptr[i] = e`: the target is the pointee type.
                    // An origin-bearing pointer designates one value and its
                    // provenance must carry mutable capability.
                    TyKind::Pointer { element, origin } => {
                        self.check_pointer_offset(origin, index)?;
                        self.check_pointer_write(object, origin)?;
                        (*element).clone()
                    }
                    // A SIMD lane write `v[i] = e`: the target is the width-1 scalar.
                    TyKind::Simd { dtype, .. } => simd_lane(dtype),
                    _ => return Err(TypeError::NotIndexable(obj_ty.to_string())),
                };
                let idx_ty = self.infer(index)?;
                self.check_subscript_index(&obj_ty, &idx_ty, "index")?;
                Ok(match elem.kind() {
                    TyKind::Ref(reference) => reference.referent.clone(),
                    _ => elem,
                })
            }
            ExprKind::Slice { object, .. } => {
                let object_type = self.check_place(object)?;
                match self.subscript_lvalue(place)? {
                    Some(lvalue) => self.subscript_reference_place(place, &object_type, &lvalue),
                    None => Err(TypeError::NotIndexable(object_type.to_string())),
                }
            }
            ExprKind::MultiIndex { object, .. } => {
                // `ptr[unsafe_offset=i] = e` (current Mojo's keyword
                // dereference): the pointer store, exactly like `ptr[i] = e`.
                if let Some((object, offset)) =
                    super::places::pointer_offset_keyword_subscript(place)
                    && let TyKind::Pointer { element, origin } = (self.infer(object)?).kind()
                {
                    self.check_pointer_offset(origin, offset)?;
                    self.check_pointer_write(object, origin)?;
                    let idx_ty = self.infer(offset)?;
                    if !self.is_index_type(&idx_ty) {
                        return Err(TypeError::TypeMismatch {
                            expected: "Indexer".to_string(),
                            found: idx_ty.to_string(),
                            context: "index".to_string(),
                        });
                    }
                    return Ok(match element.kind() {
                        TyKind::Ref(reference) => reference.referent.clone(),
                        _ => (*element).clone(),
                    });
                }
                let object_type = self.check_place(object)?;
                reject_keyword_subscript_store(place)?;
                match self.subscript_lvalue(place)? {
                    Some(lvalue) => self.subscript_reference_place(place, &object_type, &lvalue),
                    None => Err(TypeError::NotIndexable(object_type.to_string())),
                }
            }
            ExprKind::TypeApply { name, args } => {
                self.check_capture_access(name, true)?;
                if !self.is_binding_mutable(name) {
                    return Err(TypeError::ImmutableBinding(name.clone()));
                }
                let variant = self
                    .lookup(name)
                    .cloned()
                    .ok_or_else(|| TypeError::UndefinedVariable(name.clone()))?;
                let TyKind::Variant(alternatives) = variant.kind() else {
                    return Err(TypeError::InvalidAssignTarget(name.clone()));
                };
                let (index, alternative) = self.variant_alternative(alternatives, args)?;
                self.record_at(place.source_span()).adjustment = Some(Box::new(
                    mojito_checked::checked::SemanticAdjustment::VariantProject {
                        alternatives: alternatives.clone(),
                        index,
                    },
                ));
                if let Some(owner) = self.lookup_owner(name) {
                    self.record_at(place.source_span()).binding = Some(owner);
                }
                Ok(alternative)
            }
            // A call returning a mutable reference (`bump(k) = 9`,
            // `bump(k) += 1`, `w.at[0]() = 5`): its result handle is the
            // place, written through and never read out as a value.
            ExprKind::MethodCall { .. } | ExprKind::Call { .. } | ExprKind::Invoke { .. } => {
                let found = self.infer(place)?;
                let callee = match &place.kind {
                    ExprKind::MethodCall { method, .. } => format!("method call '{method}()'"),
                    ExprKind::Call { name, .. } => format!("call '{name}()'"),
                    _ => "the call".to_string(),
                };
                let span = place.source_span();
                let reference = match found.kind() {
                    TyKind::Ref(reference) => Some(reference.clone()),
                    _ => self
                        .node_facts
                        .borrow()
                        .get(&span)
                        .and_then(|facts| facts.selected_call.as_deref())
                        .and_then(|contract| contract.reference_result.clone())
                        .or_else(|| {
                            match self
                                .node_facts
                                .borrow()
                                .get(&span)
                                .and_then(|facts| facts.adjustment.as_deref())
                            {
                                Some(
                                    mojito_checked::checked::SemanticAdjustment::ReferenceResult {
                                        reference,
                                    },
                                ) => Some(reference.clone()),
                                _ => None,
                            }
                        }),
                };
                match reference {
                    Some(reference)
                        if reference.mutability != mojito_types::origin::Mutability::Immutable =>
                    {
                        self.record_at(span).reference_value_use = Some(true);
                        Ok(reference.referent)
                    }
                    Some(_) => Err(TypeError::ImmutableBinding(format!(
                        "immutable reference returned by {callee}"
                    ))),
                    None => Err(TypeError::InvalidAssignTarget(format!(
                        "the temporary result of {callee}"
                    ))),
                }
            }
            // Upstream's `emitLValue` rejects any other expression.
            _ => Err(TypeError::InvalidAssignTarget(
                "expression must be mutable in assignment".to_string(),
            )),
        }
    }

    /// The checker-only argument standing for a slice literal with
    /// `operands` (lower, upper, stride) while selecting
    /// `__getitem__`/`__setitem__`: upstream's slice literal, an initializer
    /// that becomes a call of its parameter type's `__slice_literal__`
    /// constructor once overload resolution binds it
    /// ([`Self::slice_literal_binding`]). It is typed as the literal's
    /// default type, which an inferred parameter takes.
    ///
    /// The argument's identity is derived from the subscript's and the
    /// literal's position in it, so every check of one subscript keys the
    /// literal alike, and a clone's copy of it traces to the template's.
    pub(super) fn synthetic_slice_descriptor(
        &self,
        span: &SourceSpan,
        position: usize,
        operands: [Option<&Expr>; 3],
    ) -> Result<Expr, TypeError> {
        for operand in operands.into_iter().flatten() {
            self.infer(operand)?;
        }
        let mut expression = Expr::new(ExprKind::None, span.span);
        expression.source.clone_from(&span.source);
        if let Some(subscript) = span.syntax {
            expression.syntax_id = mojito_common::token::SyntaxId::derived(
                subscript,
                u32::try_from(position).unwrap_or(u32::MAX),
            );
        }
        self.slice_literals.borrow_mut().insert(
            expression.source_span(),
            operands.map(Option::<&Expr>::cloned),
        );
        Ok(expression)
    }

    /// Whether `expression` is the checker-only value standing in for a
    /// slice literal ([`Self::synthetic_slice_descriptor`]). It binds a
    /// parameter only through that parameter type's `__slice_literal__`
    /// constructor, never through a user `@implicit` constructor of its
    /// default type, and no conversion is recorded on it.
    pub(super) fn is_synthetic_slice_descriptor(&self, expression: &Expr) -> bool {
        self.slice_literal_type(expression).is_some()
    }

    /// The type of the checker-only value standing in for a slice literal
    /// ([`Self::synthetic_slice_descriptor`]): the literal's default type,
    /// which an inferred parameter takes. `None` for any other expression.
    pub(super) fn slice_literal_type(&self, expression: &Expr) -> Option<Ty> {
        if !matches!(expression.kind, ExprKind::None) {
            return None;
        }
        let stride = self
            .slice_literals
            .borrow()
            .get(&expression.source_span())?[2]
            .clone();
        Some(Self::slice_literal_default_type(stride.as_ref()))
    }

    /// How the slice literal `literal` binds a parameter of type `expected`,
    /// if it does, as upstream's `T(lower, upper, stride,
    /// __slice_literal__=None)` for a parameter of type `T`, an omitted
    /// operand passing `None`. An inferred parameter binds the literal's
    /// default type. When `record`, each written operand's conversion to its
    /// constructor parameter is recorded on it.
    pub(super) fn slice_literal_binding(
        &self,
        literal: &Expr,
        expected: &Ty,
        record: bool,
    ) -> Result<Option<mojito_checked::checked::CheckedSliceLiteral>, TypeError> {
        let Some(operands) = self
            .slice_literals
            .borrow()
            .get(&literal.source_span())
            .cloned()
        else {
            return Ok(None);
        };
        let target = match expected.kind() {
            TyKind::Param { .. } => {
                let default = Self::slice_literal_default_type(operands[2].as_ref());
                if !self.value_coerces(&default, expected) {
                    return Ok(None);
                }
                default
            }
            _ => expected.clone(),
        };
        self.slice_literal_construction(&target, &operands, record)
    }

    /// The nominal subscript `target` as an assignment target, as upstream's
    /// `emitGetterSetterAccess` forms it. `None` for a receiver that is not a
    /// declared struct and for a type-keyed projection (upstream's
    /// `__getitem_param__`, which precedes the accessor lookup). A setter
    /// requires a getter: the getter is resolved over the indices, without
    /// being called, and its result is the element type every store converts
    /// its value to.
    pub(super) fn subscript_lvalue(
        &self,
        target: &Expr,
    ) -> Result<Option<SubscriptLValue>, TypeError> {
        let object = match &target.kind {
            ExprKind::Index { object, index } if self.type_keyed_projection(object, index) => {
                return Ok(None);
            }
            ExprKind::Index { object, .. }
            | ExprKind::Slice { object, .. }
            | ExprKind::MultiIndex { object, .. } => object,
            _ => return Ok(None),
        };
        let receiver = self.infer(object)?;
        let receiver = match receiver.kind() {
            TyKind::Ref(reference) => reference.referent.clone(),
            _ => receiver,
        };
        let TyKind::Struct(name, _) = receiver.kind() else {
            return Ok(None);
        };
        let Some(info) = self.structs.get(name) else {
            return Ok(None);
        };
        let has_getter = info.methods.contains_key("__getitem__");
        // Upstream emits a static getter (`__getitem_param__`) before it
        // looks up the accessors: its result is the place.
        if !has_getter && info.methods.contains_key("__getitem_param__") {
            return Ok(Some(SubscriptLValue::Getter));
        }
        let Some(setters) = info.methods.get("__setitem__") else {
            return if has_getter {
                Ok(Some(SubscriptLValue::Getter))
            } else {
                Err(TypeError::NotIndexable(receiver.to_string()))
            };
        };
        if !has_getter {
            return Err(TypeError::SetterWithoutGetter(receiver.to_string()));
        }
        let value_name =
            setters
                .first()
                .and_then(setter_value_name)
                .ok_or_else(|| TypeError::BadCall {
                    func: format!("{name}.__setitem__"),
                    reason: "__setitem__ must take at least one argument for the value to set"
                        .to_string(),
                })?;
        let saved = self.resolving_accessor_only.replace(true);
        let getter = self.probe(|| {
            let element = self.infer(target)?;
            let returns_reference = self
                .fact(&target.source_span(), |facts| {
                    facts.selected_call.as_deref()
                })
                .is_some_and(|getter| getter.reference_result.is_some());
            Ok((element, returns_reference))
        });
        self.resolving_accessor_only.set(saved);
        let (element, getter_returns_ref) = getter?;
        Ok(Some(SubscriptLValue::GetterSetter {
            element,
            getter_returns_ref,
            value_name,
        }))
    }

    /// Check `target = value` for a nominal subscript `target`, as
    /// upstream's `emitAssign` stores into a `SubscriptDLValue`: the value is
    /// typed at the getter's element type, then passed to `__setitem__`
    /// under the setter's value name. `Ok(None)` leaves primitive
    /// pointer/SIMD and compiler-private storage assignments on their
    /// intrinsic place path.
    pub(super) fn check_nominal_subscript_assignment(
        &self,
        target: &Expr,
        value: &Expr,
    ) -> Result<Option<Ty>, TypeError> {
        reject_keyword_subscript_store(target)?;
        let Some(lvalue) = self.subscript_lvalue(target)? else {
            return Ok(None);
        };
        let SubscriptLValue::GetterSetter {
            element,
            value_name,
            ..
        } = lvalue
        else {
            return self
                .check_subscript_reference_assignment(target, value)
                .map(Some);
        };
        let found = self.infer_with_expected(value, &element, true)?;
        // A value already of the element type is the setter's argument
        // itself, read in place by a `read` parameter.
        if found == element {
            self.store_through_subscript(target, &value_name, value)?;
            return Ok(Some(element));
        }
        // Any other value is converted to the element type first, and the
        // converted value is what the setter is selected over and passed.
        if !self.record_implicit_conversion(value, &found, &element)? {
            return Err(TypeError::TypeMismatch {
                expected: element.to_string(),
                found: found.to_string(),
                context: "index assignment".to_string(),
            });
        }
        if !self.storage_conversion_borrows_source(&found, Some(&element)) {
            self.check_consuming(value, &found, "index assignment")?;
        }
        let converted = self.synthetic_store_value(target, &element);
        let mut setter = self.store_through_subscript(target, &value_name, &converted)?;
        let site = target.source_span();
        self.release_synthetic_store_value(&site, &converted, &mut setter);
        self.record_at(site).selected_call = Some(Box::new(setter));
        Ok(Some(element))
    }

    /// Store `value` through the subscript `target`'s `__setitem__`, as
    /// upstream's `SubscriptDLValue::emitStore` does: an ordinary method
    /// call taking the indices and `value_name=value`. The target records
    /// the element type (`value`'s) and the setter's contract, which is
    /// returned.
    pub(super) fn store_through_subscript(
        &self,
        target: &Expr,
        value_name: &str,
        value: &Expr,
    ) -> Result<mojito_checked::checked::CheckedCallContract, TypeError> {
        let (object, arguments, descriptors) = match &target.kind {
            ExprKind::Index { object, index } => {
                let object_ty = self.infer(object)?;
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
            } => {
                let descriptor = self.synthetic_slice_descriptor(
                    &target.source_span(),
                    0,
                    [lower.as_deref(), upper.as_deref(), step.as_deref()],
                )?;
                (
                    object.as_ref(),
                    vec![descriptor.clone()],
                    vec![Some(descriptor)],
                )
            }
            ExprKind::MultiIndex {
                object,
                args: source,
            } => {
                let object_ty = self.infer(object)?;
                let mut arguments = Vec::with_capacity(source.len());
                let mut descriptors = Vec::with_capacity(source.len());
                reject_keyword_subscript_store(target)?;
                for (position, argument) in source.iter().enumerate() {
                    match argument {
                        SubscriptArg::Keyword { .. } | SubscriptArg::KeywordSlice { .. } => {}
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
                        SubscriptArg::Slice { lower, upper, step } => {
                            let descriptor = self.synthetic_slice_descriptor(
                                &target.source_span(),
                                position,
                                [lower.as_deref(), upper.as_deref(), step.as_deref()],
                            )?;
                            arguments.push(descriptor.clone());
                            descriptors.push(Some(descriptor));
                        }
                    }
                }
                (object.as_ref(), arguments, descriptors)
            }
            _ => {
                return Err(TypeError::InvariantViolation(
                    "a subscript store's target is not a subscript".to_string(),
                ));
            }
        };
        let object_ty = self.infer(object)?;
        let kwargs = vec![mojito_ast::ast::KwArg {
            name: value_name.to_string(),
            value: value.clone(),
        }];
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
        self.infer_method_call(&target.source_span(), object, "__setitem__", call)?;
        let contract = self
            .fact(&target.source_span(), |facts| {
                facts.selected_call.as_deref()
            })
            .ok_or_else(|| {
                TypeError::InvariantViolation(
                    "nominal subscript assignment lost its selected call contract".to_string(),
                )
            })?;
        let element = self.infer(value)?;
        let descriptors =
            self.bind_selected_slice_literals(&target.source_span(), &[], &descriptors)?;
        self.record_at(target.source_span()).subscript_descriptor =
            Some((descriptors, Some(value_name.to_string())));
        self.record_at(target.source_span()).ty = Some(element.clone());
        self.record_at(target.source_span()).place_ty = Some(element);
        Ok(contract)
    }

    /// A value of type `ty` standing for a store's computed value (an
    /// augmented operator's result, or a right-hand side converted to the
    /// element type): no expression, so the setter is selected over `ty`.
    pub(super) fn synthetic_store_value(&self, target: &Expr, ty: &Ty) -> Expr {
        let mut value = Expr::new(ExprKind::None, mojito_common::token::DUMMY_SPAN);
        value.source.clone_from(&target.source);
        self.record_at(value.source_span()).ty = Some(ty.clone());
        value
    }

    /// Forget the facts the setter call recorded on the synthetic `value`
    /// ([`Self::synthetic_store_value`]): lowering binds the computed
    /// register to the setter directly, so the setter's value argument is
    /// keyed at the store's own `site` and nothing is recorded for it as a
    /// temporary argument.
    pub(super) fn release_synthetic_store_value(
        &self,
        site: &SourceSpan,
        value: &Expr,
        setter: &mut mojito_checked::checked::CheckedCallContract,
    ) {
        let value_source = value.source_span();
        self.clear_fact(&value_source, |facts| facts.implicit_conversion.take());
        self.clear_fact(&value_source, |facts| facts.conversion_source_borrow.take());
        self.clear_fact(&value_source, |facts| facts.ty.take());
        if matches!(
            self.node_facts
                .borrow()
                .get(&value_source)
                .and_then(|facts| facts.adjustment.as_deref()),
            Some(mojito_checked::checked::SemanticAdjustment::MaterializeLiteral(_))
        ) {
            self.clear_fact(&value_source, |facts| facts.adjustment.take());
        }
        self.clear_flag(&value_source, |facts| &mut facts.read_temporary_argument);
        self.unconsumed_temporaries
            .borrow_mut()
            .remove(&value_source);
        for argument in &mut setter.boundary.arguments {
            if argument.value_source == value_source {
                argument.value_source.clone_from(site);
            }
        }
    }

    /// The nominal subscript `place`, of a `receiver` whose accessors form
    /// `lvalue`, as a place: the getter's mutable reference result, written
    /// through as a call returning one is.
    fn subscript_reference_place(
        &self,
        place: &Expr,
        receiver: &Ty,
        lvalue: &SubscriptLValue,
    ) -> Result<Ty, TypeError> {
        if let SubscriptLValue::GetterSetter {
            getter_returns_ref: false,
            ..
        } = lvalue
        {
            return Err(TypeError::Unsupported(format!(
                "a value '__getitem__' result of '{receiver}' used as a mutable place is \
                 written back through '__setitem__' (get, modify, set); not supported yet"
            )));
        }
        let referent = self.infer(place)?;
        let span = place.source_span();
        let mutability = self
            .fact(&span, |facts| facts.selected_call.as_deref())
            .and_then(|contract| contract.reference_result)
            .map(|reference| reference.mutability);
        match mutability {
            Some(mutability) if mutability != mojito_types::origin::Mutability::Immutable => {
                self.record_at(span).reference_value_use = Some(true);
                Ok(referent)
            }
            Some(_) => Err(TypeError::ImmutableBinding(
                "immutable reference returned by '__getitem__'".to_string(),
            )),
            None => Err(TypeError::ImmutableBinding(format!(
                "the value a subscript of '{receiver}' returns"
            ))),
        }
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
    ) -> Result<Ty, TypeError> {
        let ExprKind::Index { object, .. } = &target.kind else {
            return Err(TypeError::Unsupported(
                "a slice or multi-index store through '__getitem__''s reference result is not \
                 supported; declare '__setitem__'"
                    .to_string(),
            ));
        };
        let object_ty = self.infer(object)?;
        let element = self.infer(target)?;
        let site = target.source_span();
        let getter = self
            .fact(&site, |facts| facts.selected_call.as_deref())
            .ok_or_else(|| {
                TypeError::InvariantViolation(
                    "subscript reference assignment lost its getter contract".to_string(),
                )
            })?;
        let Some(reference) = &getter.reference_result else {
            // A value-returning getter offers no write-through path; without a
            // setter its result is no place.
            return Err(TypeError::ImmutableBinding(format!(
                "the value a subscript of '{object_ty}' returns"
            )));
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
        self.record_at(site.clone())
            .subscript_descriptor
            .get_or_insert_with(|| (vec![None], None));
        self.record_at(site.clone()).ty = Some(element.clone());
        self.record_at(site.clone()).place_ty = Some(element.clone());
        self.record_at(site).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::AugmentedSubscript(Box::new(
                mojito_checked::checked::CheckedAugmentedSubscript {
                    getter,
                    setter: None,
                    inplace: None,
                    operand_ty: element.clone(),
                    result_ty: element.clone(),
                },
            )),
        ));
        Ok(element)
    }

    pub(super) fn infer_slice_subscript(
        &self,
        span: SourceSpan,
        object: &Expr,
        lower: Option<&Expr>,
        upper: Option<&Expr>,
        step: Option<&Expr>,
    ) -> Result<Ty, TypeError> {
        let object_type = self.infer(object)?;
        let (result, literal) = match object_type.kind() {
            // Positional slicing on a StringLiteral value is gone at the
            // audited head along with nominal String positional slicing;
            // only the keyword-unit spellings remain.
            TyKind::StringLiteral => {
                return Err(TypeError::Unsupported(
                    "String positional slicing was removed in current Mojo; spell the \
                     unit explicitly: 's[byte=a:b]' or 's[codepoint=a:b]'"
                        .to_string(),
                ));
            }
            // Positional String slicing was removed in current Mojo (like
            // bare positional `s[i]`): the byte or codepoint unit is spelled
            // explicitly through a keyword slice.
            TyKind::Struct(name, arguments)
                if arguments.is_empty() && mojito_symbol::symbol::is_stdlib_string_struct(name) =>
            {
                return Err(TypeError::Unsupported(
                    "String positional slicing was removed in current Mojo; spell the \
                     unit explicitly: 's[byte=a:b]' or 's[codepoint=a:b]'"
                        .to_string(),
                ));
            }
            TyKind::Struct(..) | TyKind::Param { .. } => {
                let descriptor = self.synthetic_slice_descriptor(&span, 0, [lower, upper, step])?;
                let result = self.infer_method_call(
                    &span,
                    object,
                    "__getitem__",
                    MethodCallArguments::ordinary(std::slice::from_ref(&descriptor), &[]),
                )?;
                (result, descriptor)
            }
            _ => return Err(TypeError::NotIndexable(object_type.to_string())),
        };
        self.record_borrowed_view_result(&span, &result);
        let descriptors = self.bind_selected_slice_literals(&span, &[], &[Some(literal)])?;
        self.record_at(span).subscript_descriptor = Some((descriptors, None));
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
        if let TyKind::Pointer { element, origin } = object_type.kind() {
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
                self.materialize_temporary_holder(object, false);
                return Ok((*element).clone());
            }
            return Err(TypeError::Unsupported(
                "a Pointer keyword subscript is spelled exactly \
                 'pointer[unsafe_offset=i]'"
                    .to_string(),
            ));
        }
        if !matches!(
            object_type.kind(),
            TyKind::Struct(..) | TyKind::Param { .. }
        ) {
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
                SubscriptArg::Slice { lower, upper, step } => {
                    let descriptor = self.synthetic_slice_descriptor(
                        &span,
                        position,
                        [lower.as_deref(), upper.as_deref(), step.as_deref()],
                    )?;
                    actual_arguments.push(descriptor.clone());
                    descriptors.push(Some(descriptor));
                }
                // A keyword slice (`s[byte=a:b]`) binds a keyword-only
                // `__getitem__` parameter through the same structural call
                // binding as a keyword subscript.
                SubscriptArg::KeywordSlice {
                    name,
                    lower,
                    upper,
                    step,
                } => {
                    let descriptor = self.synthetic_slice_descriptor(
                        &span,
                        position,
                        [lower.as_deref(), upper.as_deref(), step.as_deref()],
                    )?;
                    keyword_arguments.push(mojito_ast::ast::KwArg {
                        name: name.clone(),
                        value: descriptor.clone(),
                    });
                    descriptors.push(Some(descriptor));
                }
            }
        }
        let result = self.infer_method_call(
            &span,
            object,
            "__getitem__",
            MethodCallArguments::ordinary(&actual_arguments, &keyword_arguments),
        )?;
        self.record_borrowed_view_result(&span, &result);
        let keyword = arguments
            .iter()
            .map(|argument| {
                matches!(
                    argument,
                    SubscriptArg::Keyword { .. } | SubscriptArg::KeywordSlice { .. }
                )
            })
            .collect::<Vec<_>>();
        let descriptors = self.bind_selected_slice_literals(&span, &keyword, &descriptors)?;
        self.record_at(span).subscript_descriptor = Some((descriptors, None));
        Ok(result)
    }

    /// Type a subscript over tuples, SIMD, lists, pointers, or a user-defined
    /// `__getitem__` implementation.
    #[allow(
        clippy::too_many_lines,
        clippy::maybe_infinite_iter,
        reason = "TODO: split this pass; the search is bounded by the enclosing collection"
    )]
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
        let obj_ty = match obj_ty.kind() {
            TyKind::Ref(reference) => reference.referent.clone(),
            _ => obj_ty,
        };
        // The empty subscript `p[]` dereferences a pointer at offset 0 (always
        // in provenance, so no offset check). A nominal receiver may declare a
        // zero-argument `__getitem__` (current Optional's raising extraction);
        // the EmptySubscript node is syntax-only and is not passed as an
        // ordinary argument.
        if matches!(index.kind, ExprKind::EmptySubscript) {
            if matches!(obj_ty.kind(), TyKind::Struct(name, _)
            if self.structs.get(name).is_some_and(|info| {
                info.methods.get("__getitem__").is_some_and(|methods| {
                    methods.iter().any(|method| method.has_self && method.params.is_empty())
                })
            })) {
                return self.infer_method_call(
                    &span,
                    object,
                    "__getitem__",
                    MethodCallArguments::ordinary(&[], &[]),
                );
            }
            return match obj_ty.kind() {
                TyKind::Pointer { element, .. } => {
                    self.materialize_temporary_holder(object, false);
                    Ok((*element).clone())
                }
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
        // Current Mojo permits a general compile-time parameter subscript hook,
        // not only the dependent accessor family synthesized for variadic
        // structs. Pass the source index as a checked value parameter and no
        // runtime argument; ordinary generic-method specialization then leaves
        // an exact callable target for HIR/MIR.
        if let TyKind::Struct(name, _) = obj_ty.kind()
            && self
                .structs
                .get(name)
                .is_some_and(|info| info.methods.contains_key("__getitem_param__"))
        {
            let parameter_arguments = [mojito_ast::ast::ParamArg::Value(index.clone())];
            let result = self.infer_method_call(
                &span,
                object,
                "__getitem_param__",
                MethodCallArguments::parameterized(&parameter_arguments, &[], &[]),
            )?;
            return Ok(match result.kind() {
                TyKind::Ref(reference) => reference.referent.clone(),
                _ => result,
            });
        }
        // A tuple is heterogeneous, so its index must be a **compile-time** `Int`
        // constant — the result type is that element's type.
        let tuple_elements = tuple_elements(&obj_ty).or_else(|| match obj_ty.kind() {
            // Compiler-private heterogeneous pack storage uses the same static
            // per-index typing rule as public Tuple, but never method dispatch.
            TyKind::Tuple(elements) => Some(elements.iter().collect()),
            _ => None,
        });
        // Storage over a pack that is still a parameter: the dependent
        // element at a compile-time index.
        if let Some([pack]) = tuple_elements.as_deref()
            && let TyKind::Param { binder, .. } = pack.kind()
            && binder.name.starts_with('*')
            && let Some(elems) = &tuple_elements
        {
            return self.pack_element_type(elems[0], index);
        }
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
            return Ok(match elems[i as usize].kind() {
                TyKind::Ref(reference) => reference.referent.clone(),
                _ => elems[i as usize].clone(),
            });
        }
        // A homogeneous runtime variadic collector is compiler-private Tuple
        // storage with one repeated element type. Unlike a heterogeneous pack,
        // its index may vary at runtime without changing the result type.
        // A heterogeneous pack that is still a parameter (`*args: *Ts`) has
        // one dependent element type per compile-time index.
        if let TyKind::VariadicPack(element) = obj_ty.kind()
            && let Some(pack) = mojito_types::types::pack_spread(std::slice::from_ref(element))
        {
            return self.pack_element_type(pack, index);
        }
        if let TyKind::VariadicPack(element) = obj_ty.kind() {
            let idx_ty = self.infer(index)?;
            if !self.is_index_type(&idx_ty) {
                return Err(TypeError::TypeMismatch {
                    expected: "Indexer".to_string(),
                    found: idx_ty.to_string(),
                    context: "variadic-pack index".to_string(),
                });
            }
            return Ok(match element.kind() {
                TyKind::Ref(reference) => reference.referent.clone(),
                _ => element.clone(),
            });
        }
        if let TyKind::Struct(name, _) = obj_ty.kind()
            && self.is_abstract_struct(name)
        {
            // A discovery-round abstract scalar range subscripts by Int to
            // its scalar element; the fixpoint rewrite constructs the
            // linked struct, with its ordinary `__getitem__`, before any
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
                return Ok(match element.clone().kind() {
                    TyKind::Ref(reference) => reference.referent.clone(),
                    _ => element.clone(),
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
                self.record_interior_reference(span, object, "value");
                return Ok(match value.clone().kind() {
                    TyKind::Ref(reference) => reference.referent.clone(),
                    _ => value.clone(),
                });
            }
        }
        // A user struct with `__getitem__` is subscriptable: `c[i]` →
        // `c.__getitem__(i)`, typed by the method (the index need not be `Int`).
        if matches!(obj_ty.kind(), TyKind::Struct(..)) {
            if let TyKind::Struct(name, _) = obj_ty.kind()
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
                return Ok(match result.kind() {
                    TyKind::Ref(reference) => reference.referent.clone(),
                    _ => result,
                });
            }
            return Err(TypeError::NotIndexable(obj_ty.to_string()));
        }
        // Bounded type parameters use the same method-selection path as
        // concrete nominal receivers. This retains the requirement's error,
        // convention, capture, and reference-origin contract instead of
        // projecting only its parameter/result types.
        if matches!(obj_ty.kind(), TyKind::Param { .. }) {
            return self.infer_method_call(
                &span,
                object,
                "__getitem__",
                MethodCallArguments::ordinary(std::slice::from_ref(index), &[]),
            );
        }
        // The result of indexing: a SIMD lane, a List element, or a pointer pointee.
        let result = match obj_ty.kind() {
            TyKind::Simd { dtype, .. } => simd_lane(dtype),
            TyKind::Pointer { element, origin } => {
                self.check_pointer_offset(origin, index)?;
                self.materialize_temporary_holder(object, false);
                (*element).clone()
            }
            _ => return Err(TypeError::NotIndexable(obj_ty.to_string())),
        };
        let idx_ty = self.infer(index)?;
        self.check_subscript_index(&obj_ty, &idx_ty, "index")?;
        Ok(match result.kind() {
            TyKind::Ref(reference) => reference.referent.clone(),
            _ => result,
        })
    }

    /// Select and record the nominal `__getitem__` contract for a struct
    /// subscript read — the shared entry for `infer_index`'s tail and the bare
    /// element-call re-dispatch, generalized to the multi-index arity. Records
    /// the interior-reference fact for list/dict-backed receivers and returns
    /// the getter's raw result type; callers read through `TyKind::Ref`.
    pub(super) fn infer_struct_getitem_call(
        &self,
        span: SourceSpan,
        object: &Expr,
        indices: &[Expr],
        obj_ty: &Ty,
    ) -> Result<Ty, TypeError> {
        let result = self.infer_method_call(
            &span,
            object,
            "__getitem__",
            MethodCallArguments::ordinary(indices, &[]),
        )?;
        // A view-typed result (a `Span` over a `ContiguousSlice` index)
        // stays borrowed from its receiver's sources.
        if !matches!(result.kind(), TyKind::Ref(_)) && self.type_carries_loans(&result) {
            self.record_borrowed_view_result(&span, &result);
            return Ok(result);
        }
        if list_element(obj_ty).is_some() {
            self.record_interior_reference(span, object, "element");
        } else if dict_elements(obj_ty).is_some() {
            self.record_interior_reference(span, object, "value");
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

    /// Reject a write through `pointer` unless its provenance carries
    /// mutable capability at this site: an immutable provenance, and a
    /// `Origin[mut=m]` binder that no binding resolves (the generic body
    /// itself, a `Pointer[T, o]` parameter), both reject — upstream judges
    /// the binder per instantiation and never inside the generic body.
    pub(super) fn check_pointer_write(
        &self,
        pointer: &Expr,
        origin: &mojito_types::origin::PointerOrigin,
    ) -> Result<(), TypeError> {
        self.materialize_temporary_holder(pointer, true);
        match self.pointer_write_capability(pointer, origin) {
            Some(true) => Ok(()),
            Some(false) => Err(TypeError::ImmutableBinding(
                "write through a Pointer whose origin is immutable".to_string(),
            )),
            None => Err(TypeError::ImmutableBinding(
                "write through a Pointer whose origin mutability is not known here".to_string(),
            )),
        }
    }

    /// Whether writes through `pointer` are permitted here: `Some` when the
    /// provenance's capability is known, `None` when it stays symbolic. A
    /// symbolic `origin_of(self)` takes the receiver's own mutability, and a
    /// pointer field's `Origin[mut=m]` binder resolves through the holder's
    /// construction-time origins (`resolve_receiver_origin_arguments`): the
    /// place those name is writable or not, while a resolution that only
    /// reaches the holder's own storage found no binding and stays symbolic.
    pub(super) fn pointer_write_capability(
        &self,
        pointer: &Expr,
        origin: &mojito_types::origin::PointerOrigin,
    ) -> Option<bool> {
        use mojito_types::origin::{Origin, PointerOrigin};
        if let Some(known) = origin.statically_mutable() {
            return Some(known);
        }
        match origin {
            PointerOrigin::SelfPlace { .. } => Some(self.self_mutable),
            PointerOrigin::Param { id, .. } => {
                let ExprKind::Member { object, .. } = &pointer.kind else {
                    return None;
                };
                if self.immutable_origin_binders(object).contains(id) {
                    return Some(false);
                }
                let resolved = self.resolve_receiver_origin_arguments(Origin::Param(*id), object);
                if self
                    .origin_place(object)
                    .is_ok_and(|place| super::origins::origin_rooted_at(&resolved, place.root))
                {
                    return None;
                }
                self.origin_writably_rooted(&resolved)
            }
            PointerOrigin::Place { .. }
            | PointerOrigin::Static
            | PointerOrigin::Untracked { .. }
            | PointerOrigin::UnsafeAny { .. } => None,
        }
    }

    /// Reject any pointer dereference on the way to a written place whose
    /// provenance is not writable, as `check_place` does for a plain
    /// assignment; a nominal subscript's getter/setter path does not walk
    /// its receiver chain as a place.
    pub(super) fn check_pointer_derefs(&self, place: &Expr) -> Result<(), TypeError> {
        match &place.kind {
            ExprKind::Index { object, .. }
            | ExprKind::Slice { object, .. }
            | ExprKind::MultiIndex { object, .. } => {
                if let TyKind::Pointer { origin, .. } = (self.infer(object)?).kind() {
                    return self.check_pointer_write(object, origin);
                }
                self.check_pointer_derefs(object)
            }
            ExprKind::Member { object, .. } => self.check_pointer_derefs(object),
            _ => Ok(()),
        }
    }

    /// Check a primitive subscript's index type. Every receiver but one
    /// normalizes an `Indexer` through `__mlir_index__`
    /// (`assets/ok/indexer_normalization.mojo`); a `SIMD` lane index is the
    /// exception, because upstream's builtin `SIMD.__getitem__`/`__setitem__`
    /// take a plain `Int` and reject a conformer
    /// (`assets/type_error/simd_subscript_indexer.mojo`).
    pub(super) fn check_subscript_index(
        &self,
        receiver: &Ty,
        idx_ty: &Ty,
        context: &str,
    ) -> Result<(), TypeError> {
        let lane = matches!(receiver.kind(), TyKind::Simd { .. });
        let accepted = if lane {
            coerces(idx_ty, &Ty::int())
        } else {
            self.is_index_type(idx_ty)
        };
        if accepted {
            return Ok(());
        }
        Err(TypeError::TypeMismatch {
            expected: if lane { "Int" } else { "Indexer" }.to_string(),
            found: idx_ty.to_string(),
            context: context.to_string(),
        })
    }

    /// Whether a value can be normalized to an `Int` index. A numeric
    /// literal or `Int` is one already; an `Indexer` type parameter or
    /// struct supplies `__mlir_index__() -> __mlir_type.index`, which
    /// upstream's `index()` casts to `Int`.
    pub(super) fn is_index_type(&self, ty: &Ty) -> bool {
        coerces(ty, &Ty::int())
            || matches!(ty.kind(), TyKind::Param { .. } | TyKind::Struct(..))
                && self.conforms_to(ty, "Indexer")
    }

    /// Resolve the exact normalization method for a non-Int `Indexer`. The
    /// abstract trait symbol is retained for a bounded generic and retargeted by
    /// ordinary checked method dispatch once its runtime receiver is known.
    pub(super) fn index_normalization_target(&self, ty: &Ty) -> Option<String> {
        match ty.kind() {
            TyKind::Struct(name, arguments) if self.conforms_to(ty, "Indexer") => {
                let info = self.structs.get(name)?;
                let substitution = struct_subst(&info.decls, arguments);
                let methods = info.methods.get("__mlir_index__")?;
                let selected = methods.iter().find(|method| {
                    method.has_self
                        && method.params.is_empty()
                        && substitute(&method.ret, &substitution) == Ty::index()
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
            TyKind::Param { .. } => {
                let methods =
                    self.lookup_trait_methods(&self.refined_bound(ty)?, "__mlir_index__", 0);
                let selected = methods
                    .iter()
                    .find(|method| method.params.is_empty() && method.ret == Ty::index())?;
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
        *expected == *TyKind::INT
            && self
                .node_facts
                .borrow()
                .get(&expression.source_span())
                .and_then(|facts| facts.implicit_conversion.as_ref())
                .is_some_and(|target| mojito_symbol::symbol::is_index_normalization_symbol(target))
    }

    /// Prepare a subscript argument for checked lowering. A nominal receiver
    /// normalizes an Indexer only when no `__getitem__` or `__setitem__`
    /// candidate accepts its source type at this argument position and an Int
    /// candidate exists there, preserving direct user overloads. Positions in a
    /// `*indices` tail use the variadic element contract. Primitive pointer and
    /// homogeneous private-pack indexing has a fixed Int contract and therefore
    /// always records the checked normalization fallback; a SIMD lane index
    /// does not, because upstream's `SIMD.__getitem__` takes a plain `Int`
    /// (see [`Self::check_subscript_index`]).
    pub(super) fn prepare_index_argument(
        &self,
        receiver: &Ty,
        expression: &Expr,
        method: &str,
        position: usize,
    ) -> Result<(), TypeError> {
        let actual = self.infer(expression)?;
        if coerces(&actual, &Ty::int()) {
            return Ok(());
        }
        let Some(target) = self.index_normalization_target(&actual) else {
            return Ok(());
        };
        let TyKind::Struct(name, arguments) = receiver.kind() else {
            if matches!(
                receiver.kind(),
                TyKind::Pointer { .. } | TyKind::VariadicPack(_)
            ) {
                self.record_at(expression.source_span()).implicit_conversion = Some(target);
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
                Some(variadic_index) if position >= variadic_index => signature.variadic.as_ref(),
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
            accepts_int |= expected == *TyKind::INT;
        }
        if !accepts_source && accepts_int {
            self.record_at(expression.source_span()).implicit_conversion = Some(target);
        }
        Ok(())
    }

    /// Type a field access `object.field`. On a generic struct value the field
    /// type has the struct's type arguments substituted in (`Pair[Int].left :
    /// Int`).
    pub(super) fn infer_member(
        &self,
        span: SourceSpan,
        object: &Expr,
        field: &str,
    ) -> Result<Ty, TypeError> {
        // `Self.n` reads the enclosing struct's value parameter.
        if let ExprKind::Identifier(s) = &object.kind
            && s == "Self"
        {
            if let Some(self_ty) = &self.self_ty {
                self.record_at(object.source_span()).ty = Some(self_ty.clone());
            }
            if self.self_value_pack(field).is_some()
                && let Some(element) = self
                    .self_decls
                    .iter()
                    .find(|d| d.name().trim_start_matches('*') == field)
                    .and_then(ParamDecl::value_element_ty)
            {
                return Ok(TyKind::VariadicPack(element.clone()).into());
            }
            return match self.self_decls.iter().find(|d| d.name() == field) {
                // A value parameter reads as its declared type: upstream's
                // `kgen.param.constant` of the binder, never a read of a
                // `Self` value.
                Some(decl @ ParamDecl::Value { ty, .. }) => {
                    self.record_member_value(
                        span,
                        super::annotations::value_parameter_expr(decl, ty),
                    );
                    Ok((*ty).clone())
                }
                _ => self
                    .read_type_member(span, object, field)
                    .unwrap_or_else(|| Err(TypeError::UnknownSelfParam(field.to_string()))),
            };
        }
        if let Some(dtype) = self.dtype_constant(object, field) {
            self.record_at(span).adjustment = Some(Box::new(
                mojito_checked::checked::SemanticAdjustment::DtypeConstant { dtype: dtype? },
            ));
            return Ok(Ty::dtype());
        }
        if field == "length"
            && let Some(ty) = self.member_type_operand(object)
            && let Some(read) = self.simd_length_read(span.clone(), &ty)
        {
            return Ok(read);
        }
        // The length of a `TypeList` value, or of a list computed from
        // packs, is the `Int` its elements fix. A type pack's own `length`
        // is its `TypeList`'s member, read below.
        if field == "length"
            && self.type_pack_named(object).is_none()
            && let Some(list) = self.type_list_operand(object)?
        {
            let value = self.param_context.list_length(&list).map_err(param_error)?;
            self.record_at(span).adjustment = Some(Box::new(
                mojito_checked::checked::SemanticAdjustment::ParamValue {
                    value,
                    materialized: None,
                },
            ));
            return Ok(Ty::int());
        }
        if let Some(read) = self.read_type_member(span.clone(), object, field) {
            return read;
        }
        // A field of a compile-time binding (`q.a` in `def f[q: Q]()`) is
        // upstream's struct extract on the parameter attribute: a parameter
        // expression read as a runtime value, never a read of the whole
        // binding.
        if self.comptime_positions.get() == 0
            && self.is_parameter_read(object)
            && matches!((self.infer(object)?).kind(), TyKind::Struct(_, arguments) if arguments.is_empty())
            && let Some(value) = self.struct_value_field(object, field)
        {
            let value = value?;
            if let Some(ty) = value.meta().as_value().cloned() {
                self.parameter_field_objects
                    .borrow_mut()
                    .insert(object.source_span());
                self.consume_parameter_read(object);
                self.record_at(span).adjustment = Some(Box::new(
                    mojito_checked::checked::SemanticAdjustment::ParamValue {
                        value,
                        materialized: None,
                    },
                ));
                return Ok(ty);
            }
        }
        let obj_ty = self.infer(object)?;
        // Projecting a field through a reference-returning expression borrows
        // that referent for the projection; it does not first create an owned
        // copy of the whole value. Retain the handle exactly as method and
        // chained-subscript receivers do, including for linear referents.
        if let Some(reference) = self.infer_reference_value(object) {
            self.record_at(object.source_span()).reference_value_use =
                Some(reference.mutability == mojito_types::origin::Mutability::Mutable);
        }
        if field == "length"
            && let Some(read) = self.simd_length_read(span.clone(), &obj_ty)
        {
            return Ok(read);
        }
        // `v._mlir_value` is upstream's `!kgen.simd` storage field, which
        // `__mlir_type[`!kgen.simd<`, ...]` resolves to the vector itself.
        if field == mojito_types::ct::MLIR_VALUE_FIELD && simd_slots(&obj_ty).is_some() {
            self.record_at(span).adjustment = Some(Box::new(
                mojito_checked::checked::SemanticAdjustment::SimdStorage,
            ));
            return Ok(obj_ty);
        }
        // `v.dtype` is upstream's lane-dtype parameter, the slot itself.
        if field == "dtype"
            && let Some(dtype) = simd_dtype(&obj_ty)
        {
            self.record_at(span).adjustment = Some(Box::new(
                mojito_checked::checked::SemanticAdjustment::DtypeConstant { dtype: dtype? },
            ));
            return Ok(Ty::dtype());
        }
        if let TyKind::Struct(sname, targs) = obj_ty.kind() {
            let info = self.structs.get(sname).ok_or_else(|| {
                TypeError::InvariantViolation(format!("struct '{sname}' was not registered"))
            })?;
            if let Some((_, fty)) = info.fields.iter().find(|(n, _)| n == field) {
                // The receiver's arguments — origin tail included — bind the
                // field's declared type (`Cell[Self.o]` on a `Wrap[origin_of(xs)]`).
                let value = substitute_at(fty, info, targs);
                return Ok(match value.kind() {
                    TyKind::Ref(reference) => reference.referent.clone(),
                    _ => value,
                });
            }
            // A compile-time value member read through an instance
            // (`args.size`) is the member of the instance's type: the
            // receiver only types the read.
            if let Some(member) = info
                .associated
                .get(field)
                .filter(|value| !matches!(value, CtValue::Type(_)))
            {
                let value = self.struct_member_at(info, targs, member)?;
                let ty = value.meta().as_value().cloned().ok_or_else(|| {
                    TypeError::InvariantViolation(format!("member '{field}' is not a value"))
                })?;
                self.record_member_value(span, value);
                return Ok(ty);
            }
        }
        // A string literal's `value` parameter is its KGEN string, which a
        // compile-time position reads and no runtime value holds.
        if field == mojito_types::ct::LITERAL_VALUE_MEMBER
            && matches!(obj_ty.kind(), TyKind::StringLiteral)
        {
            return if self.crosses_to_runtime() {
                Err(super::comptime_validation::kgen_string_runtime_read())
            } else {
                Ok(TyKind::KgenString.into())
            };
        }
        Err(TypeError::NoSuchField {
            object_type: obj_ty.to_string(),
            field: field.to_string(),
        })
    }

    /// A type's compile-time value member read as a runtime value (`T.K`,
    /// `Self.T.K`, `A.K`, `Self.K`): the parameter value
    /// [`Self::type_member_value`] builds, recorded at `span` for MIR to
    /// carry, typed by its value type. Type-valued members stay type-position
    /// only (`T.Element` in annotations).
    pub(super) fn read_type_member(
        &self,
        span: SourceSpan,
        object: &Expr,
        field: &str,
    ) -> Option<Result<Ty, TypeError>> {
        let value = match self.type_member_value(object, field)? {
            Ok(value) => value,
            Err(error) => return Some(Err(error)),
        };
        // A raw parameter list (`vs.values`) is typed where a compile-time
        // position reads it, and has no runtime form.
        if let mojito_types::param_expr::MetaTy::ParamList(_) = value.meta()
            && self.crosses_to_runtime()
        {
            return Some(Err(super::comptime_validation::raw_list_runtime_read(
                value.meta(),
            )));
        }
        let ty = value.meta().value_ty()?;
        self.record_member_value(span, value);
        Some(Ok(ty))
    }

    /// A generic alias or a struct's parameterized `comptime` member
    /// applied as a runtime value (`plus_one[k]`, `S[10].plus[2]`,
    /// `IsSmall[Int]`): the parameter value its application binds, recorded
    /// at `expr` for MIR to carry, typed by its value type.
    pub(super) fn read_parametric_value(&self, expr: &Expr) -> Option<Result<Ty, TypeError>> {
        let value = match self
            .parametric_value_application(expr)
            .or_else(|| self.predicate_alias_value(expr))?
        {
            Ok(value) => value,
            Err(error) => return Some(Err(error)),
        };
        let Some(ty) = value.meta().as_value().cloned() else {
            return Some(Err(TypeError::TypeMismatch {
                expected: "a value".to_string(),
                found: value.meta().to_string(),
                context: "a compile-time application read as a runtime value".to_string(),
            }));
        };
        self.record_member_value(expr.source_span(), value);
        Some(Ok(ty))
    }

    /// Record `value` as the type member value read at `span`.
    pub(super) fn record_member_value(&self, span: SourceSpan, value: ParamExpr) {
        self.record_at(span).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::ParamValue {
                value,
                materialized: None,
            },
        ));
    }

    /// A dtype constant read as a value: the `dtype` of a `SIMD` type operand
    /// (`Int32.dtype`), or `DType.<name>` when `DType` names the builtin
    /// rather than a binding or a struct — the dtype, an unsupported
    /// upstream-only name, or an unknown member. `None` for any other member
    /// expression.
    /// `length` of a SIMD type or value (`v.length`, `Int32.length`; a
    /// native scalar is a width-1 vector): upstream's `length` parameter, a
    /// `SIMDLength`, the length whose lane count the width slot holds. The
    /// read's type, or `None` when `ty` is no SIMD type.
    fn simd_length_read(&self, span: SourceSpan, ty: &Ty) -> Option<Ty> {
        let (_, width) = simd_slots(ty)?;
        let length = self.simd_length_parameter(&width)?;
        let read = length.meta().as_value().cloned()?;
        self.record_at(span).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::SimdLength { length },
        ));
        Some(read)
    }

    pub(super) fn dtype_constant(
        &self,
        object: &Expr,
        field: &str,
    ) -> Option<Result<SimdDtype, TypeError>> {
        if field == "dtype"
            && let Some(dtype) = self
                .member_type_operand(object)
                .as_ref()
                .and_then(simd_dtype)
        {
            return Some(dtype);
        }
        let ExprKind::Identifier(name) = &object.kind else {
            return None;
        };
        if name != "DType" || self.lookup(name).is_some() || self.structs.contains_key(name) {
            return None;
        }
        Some(
            mojito_ast::ast::Dtype::from_name(field)
                .map(SimdDtype::Known)
                .ok_or_else(|| {
                    if mojito_ast::ast::UPSTREAM_ONLY_DTYPE_NAMES.contains(&field) {
                        TypeError::Unsupported(format!("DType.{field} is not supported yet"))
                    } else {
                        TypeError::NoSuchField {
                            object_type: "DType".to_string(),
                            field: field.to_string(),
                        }
                    }
                }),
        )
    }

    /// The type a member's object names when it is a type operand (`Int32`,
    /// `SIMD[DType.int16, 4]`, `Scalar[DType.int8]`, a member application
    /// `TypeList.of[Trait=AnyType, Int]`) or a construction of one
    /// (`TypeList[Trait=AnyType, Ts.values]()`), rather than a value. `None`
    /// when its root names a binding or it names no type.
    pub(super) fn member_type_operand(&self, object: &Expr) -> Option<Ty> {
        self.member_type_operand_resolution(object)?.ok()
    }

    /// The resolution of a member's object spelled as a type operand, its
    /// error included (`Box[Self.B]` over a type member `B`); `None` when
    /// its root names a binding or `Self`, or it is not spelled as a type.
    pub(super) fn member_type_operand_resolution(
        &self,
        object: &Expr,
    ) -> Option<Result<Ty, TypeError>> {
        // A member of a type or of its application (`S[Int].Of`).
        fn applied_member_root(object: &Expr) -> Option<&String> {
            let ExprKind::Member { object, .. } = &object.kind else {
                return None;
            };
            match &object.kind {
                ExprKind::Identifier(name) | ExprKind::TypeApply { name, .. } => Some(name),
                _ => None,
            }
        }
        let root = match &object.kind {
            ExprKind::Identifier(name) => Some(name),
            ExprKind::Index { object, .. } => match &object.kind {
                ExprKind::Identifier(name) => Some(name),
                _ => Some(applied_member_root(object)?),
            },
            ExprKind::MultiIndex { object, .. } => Some(applied_member_root(object)?),
            ExprKind::TypeApply { .. } | ExprKind::TypeValue(_) => None,
            // A construction (`TypeList[Trait=AnyType, Ts.values]()`,
            // `TypeList.of[Int]()`) is read on the type it constructs.
            ExprKind::Call { name, .. }
                if self.lookup(name).is_none()
                    && (self.structs.contains_key(name)
                        || self.comptime_aliases.contains_key(name)) =>
            {
                return Some(self.compile_time_value_type(object));
            }
            ExprKind::Invoke { callee, .. }
                if matches!(&callee.kind, ExprKind::Member { object, .. }
                    if matches!(&object.kind, ExprKind::Identifier(name)
                        if name != "Self" && self.lookup(name).is_none())) =>
            {
                return Some(self.compile_time_value_type(object));
            }
            _ => return None,
        };
        if root.is_some_and(|name| name == "Self" || self.lookup(name).is_some()) {
            return None;
        }
        let source = super::constraints::assoc_body_source_type(object).ok()?;
        Some(self.ty_from_anno(&source))
    }

    /// The `DType.<method>[dtype]()` floating-point format query `callee`
    /// names with `param_args`, as the parameter value a runtime read
    /// records: its answer at a known dtype, and the application of the
    /// static method over a symbolic one, whose constraint and answer are the
    /// instantiation's. `None` for any other callee.
    pub(super) fn dtype_float_query(
        &self,
        callee: &Expr,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<Result<(ParamExpr, BracketAttributes), TypeError>> {
        let ExprKind::Member { object, field } = &callee.kind else {
            return None;
        };
        if !mojito_ast::ast::DTYPE_FLOAT_QUERIES.contains(&field.as_str())
            || !matches!(&object.kind, ExprKind::Identifier(name)
                if name == "DType" && self.lookup(name).is_none() && !self.structs.contains_key(name))
        {
            return None;
        }
        let func = format!("DType.{field}");
        let [argument] = param_args else {
            return Some(Err(TypeError::BadCall {
                func,
                reason: format!(
                    "expects one compile-time dtype argument, got {}",
                    param_args.len()
                ),
            }));
        };
        if !args.is_empty() || !kwargs.is_empty() {
            return Some(Err(TypeError::BadCall {
                func,
                reason: "takes no arguments".to_string(),
            }));
        }
        Some(self.dtype_from_arg(argument).and_then(|dtype| {
            let attributes = vec![dtype_attribute(argument, &dtype)];
            let dtype = match dtype {
                SimdDtype::Known(dtype) => self
                    .param_context
                    .constant(mojito_types::ct::CtValue::Dtype(dtype))
                    .map_err(|error| TypeError::Unsupported(error.to_string()))?,
                SimdDtype::Expr(dtype) => dtype,
            };
            self.param_context
                .answer_builtin_applications(&self.param_context.dtype_float_query(field, &dtype))
                .map(|value| (value, attributes))
                .map_err(|error| TypeError::BadCall {
                    func,
                    reason: error.to_string(),
                })
        }))
    }
}

/// The lane dtype a `SIMD` type exposes as `dtype`, where `Int` and `Float64`
/// are width-1 vectors and `UInt`'s `DType.uint` has no Mojito dtype. `None`
/// for any other type: a `Bool` or a literal type has no `dtype`.
pub(super) fn simd_dtype(ty: &Ty) -> Option<Result<SimdDtype, TypeError>> {
    match ty.kind() {
        TyKind::INT => Some(Ok(SimdDtype::Known(mojito_ast::ast::Dtype::Int))),
        TyKind::FLOAT64 => Some(Ok(SimdDtype::Known(mojito_ast::ast::Dtype::Float64))),
        TyKind::UINT => Some(Ok(SimdDtype::Known(mojito_ast::ast::Dtype::UInt))),
        TyKind::Simd { dtype, .. } => Some(Ok(dtype.clone())),
        _ => None,
    }
}

impl Checker {
    /// Record that the view-typed (`result`) subscript at `span` stays
    /// borrowed from its receiver's sources: its binding inherits the
    /// receiver's loans instead of owning fresh storage. Re-inference keeps
    /// an owner a `ref self` consumer already minted for this view
    /// temporary.
    fn record_borrowed_view_result(&self, span: &SourceSpan, result: &Ty) {
        if !self.type_carries_loans(result) {
            return;
        }
        let mut record = self.record_at(span.clone());
        let materialized = match record.adjustment.as_deref() {
            Some(mojito_checked::checked::SemanticAdjustment::BorrowViewResult {
                materialized,
                ..
            }) => *materialized,
            _ => None,
        };
        record.adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::BorrowViewResult {
                materialized,
                interior: Vec::new(),
            },
        ));
    }

    /// `literals`, a subscript's per-argument slice literals (`None` for an
    /// ordinary index), as the call selected at `span` binds them, with
    /// their operand conversions recorded. `keyword` marks the arguments
    /// spelled by keyword, in source order; a missing entry is positional.
    /// Without a selected call a literal builds its default type.
    fn bind_selected_slice_literals(
        &self,
        span: &SourceSpan,
        keyword: &[bool],
        literals: &[Option<Expr>],
    ) -> Result<Vec<Option<mojito_checked::checked::CheckedSliceLiteral>>, TypeError> {
        let contract = self.fact(span, |facts| facts.selected_call.as_deref());
        let spelled_by_keyword = |position: usize| keyword.get(position).copied().unwrap_or(false);
        let parameter = |position: usize| {
            let by_keyword = spelled_by_keyword(position);
            let index = (0..position)
                .filter(|earlier| spelled_by_keyword(*earlier) == by_keyword)
                .count();
            let source = if by_keyword {
                mojito_checked::checked::CheckedCallArgumentSource::Keyword(index)
            } else {
                mojito_checked::checked::CheckedCallArgumentSource::Positional(index)
            };
            contract.as_ref().and_then(|contract| {
                contract
                    .arguments
                    .iter()
                    .find(|argument| argument.source == source)
                    .map(|argument| argument.parameter_ty.clone())
            })
        };
        literals
            .iter()
            .enumerate()
            .map(|(position, literal)| {
                literal
                    .as_ref()
                    .map(|literal| {
                        let parameter = parameter(position).unwrap_or_else(|| {
                            self.slice_literal_type(literal).unwrap_or_else(Ty::error)
                        });
                        self.bound_slice_literal(literal, &parameter)
                    })
                    .transpose()
            })
            .collect()
    }

    /// The slice literal `literal` as the selected parameter type
    /// `parameter` binds it, recording its operand conversions.
    fn bound_slice_literal(
        &self,
        literal: &Expr,
        parameter: &Ty,
    ) -> Result<mojito_checked::checked::CheckedSliceLiteral, TypeError> {
        self.slice_literal_binding(literal, parameter, true)?
            .ok_or_else(|| {
                TypeError::InvariantViolation(format!(
                    "a slice literal does not bind its selected '{parameter}' parameter"
                ))
            })
    }

    /// The type a slice literal whose stride operand is `stride` has where
    /// no parameter type is in play, as upstream's `getDefaultType` picks
    /// it: `ContiguousSlice` when the stride is omitted or `None`, `Slice`
    /// otherwise.
    fn slice_literal_default_type(stride: Option<&Expr>) -> Ty {
        let name = if stride.is_none_or(|stride| matches!(stride.kind, ExprKind::None)) {
            "ContiguousSlice"
        } else {
            "Slice"
        };
        TyKind::Struct(
            mojito_module::module::linked_item_name("std.builtin.builtin_slice", name),
            Vec::new(),
        )
        .into()
    }

    /// The call of `target`'s `__slice_literal__` constructor a slice literal
    /// with `operands` makes, if exactly one such constructor accepts them
    /// at least cost: each written operand converts to its parameter as a
    /// call argument does, and each omitted one passes `None`. When
    /// `record`, the written operands' conversions are recorded on them.
    fn slice_literal_construction(
        &self,
        target: &Ty,
        operands: &[Option<Expr>; 3],
        record: bool,
    ) -> Result<Option<mojito_checked::checked::CheckedSliceLiteral>, TypeError> {
        let TyKind::Struct(name, arguments) = target.kind() else {
            return Ok(None);
        };
        let Some(info) = self.structs.get(name) else {
            return Ok(None);
        };
        let Some(constructors) = info.methods.get("__init__") else {
            return Ok(None);
        };
        let substitution = struct_subst(&info.decls, arguments);
        let struct_arguments =
            super::comptime_validation::positional_pack_arguments(&info.decls, arguments);
        let mut viable = Vec::new();
        for signature in constructors.iter().filter(|signature| {
            signature.decls.is_empty()
                && signature.keyword_only.is_none_or(|first| first >= 3)
                && mojito_symbol::symbol::is_slice_literal_constructor(
                    &signature.names,
                    signature.variadic.is_some(),
                )
                && signature
                    .required
                    .iter()
                    .zip(&signature.names)
                    .skip(3)
                    .all(|(required, name)| !required || name == "__slice_literal__")
        }) {
            let mut environment = ConstraintEnvironment::declared(&signature.decls, &[]);
            environment.bind_declared(&info.decls, &struct_arguments);
            if signature.availability.iter().any(|constraint| {
                self.validate_constraint_in_environment(name, constraint, &environment)
                    .is_err()
            }) {
                continue;
            }
            let parameters: Vec<Ty> = signature
                .params
                .iter()
                .take(3)
                .map(|parameter| substitute(parameter, &substitution))
                .collect();
            let mut cost = Some(0);
            for (operand, parameter) in operands.iter().zip(&parameters) {
                let found = match operand {
                    Some(operand) => self.infer_with_expected(operand, parameter, false)?,
                    None => Ty::none(),
                };
                let converts = self.value_coerces(&found, parameter)
                    || self
                        .implicit_conversion_target(&found, parameter)?
                        .is_some();
                cost = cost
                    .filter(|_| converts)
                    .map(|cost| cost + conversion_count(&found, parameter));
            }
            if let Some(cost) = cost {
                viable.push((cost, signature, parameters));
            }
        }
        let Some(best) = viable.iter().map(|(cost, ..)| *cost).min() else {
            return Ok(None);
        };
        viable.retain(|(cost, ..)| *cost == best);
        let [(_, signature, parameters)] = viable.as_slice() else {
            return Err(TypeError::BadCall {
                func: name.clone(),
                reason: "ambiguous slice-literal constructor".to_string(),
            });
        };
        if signature.raises {
            return Err(TypeError::Unsupported(format!(
                "a slice literal calling the raising '{name}' constructor is not supported"
            )));
        }
        let mut omitted = [None, None, None];
        for ((operand, parameter), omitted) in operands.iter().zip(parameters).zip(&mut omitted) {
            match operand {
                Some(operand) if record => {
                    let found = self.infer_with_expected(operand, parameter, true)?;
                    self.record_implicit_conversion(operand, &found, parameter)?;
                }
                Some(_) => {}
                None => {
                    let conversion = if self.value_coerces(&Ty::none(), parameter) {
                        None
                    } else {
                        self.implicit_conversion_target(&Ty::none(), parameter)?
                            .map(|(constructor, _)| (constructor, parameter.clone()))
                    };
                    *omitted = Some(mojito_checked::checked::OmittedSliceOperand { conversion });
                }
            }
        }
        let instance = self.self_instance_ty(name);
        let constructor = constructor_symbol(
            name,
            "__init__",
            constructors.len() > 1,
            signature,
            instance.as_ref(),
        );
        // The initializer's declared result is the struct at its own
        // parameters, as a construction's is judged.
        let indirect_result = self.memory_only(instance.as_ref().unwrap_or(target));
        Ok(Some(mojito_checked::checked::CheckedSliceLiteral {
            target: target.clone(),
            constructor,
            omitted,
            indirect_result,
        }))
    }
}

/// The name of `setter`'s last argument, which upstream takes as every
/// `__setitem__` overload's value name. `None` when it takes only `self`.
fn setter_value_name(setter: &MethodSig) -> Option<String> {
    match (setter.variadic_index, &setter.variadic_name) {
        (Some(index), Some(name)) if index == setter.names.len() => Some(name.clone()),
        _ => setter.names.last().cloned(),
    }
}

/// Reject a store to a subscript with a keyword argument (`s[byte=0] = v`):
/// a keyword subscript is read-only here.
fn reject_keyword_subscript_store(target: &Expr) -> Result<(), TypeError> {
    let ExprKind::MultiIndex { args, .. } = &target.kind else {
        return Ok(());
    };
    args.iter()
        .find_map(|argument| match argument {
            SubscriptArg::Keyword { name, .. } | SubscriptArg::KeywordSlice { name, .. } => {
                Some(name)
            }
            SubscriptArg::Index(_) | SubscriptArg::Slice { .. } => None,
        })
        .map_or(Ok(()), |name| {
            Err(TypeError::Unsupported(format!(
                "keyword subscript assignment ('[{name}=…] = value') is not supported; keyword subscripts are read-only"
            )))
        })
}

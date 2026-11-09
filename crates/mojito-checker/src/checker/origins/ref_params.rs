//! Reference-parameter handles and loan-carrying type queries,
//! including capture origins.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

impl Checker {
    /// `binder`: the enclosing struct's origin binder the parameter's clause
    /// names (`ref[Self.o] xs`), when it names one — see
    /// [`Checker::reference_parameter_struct_binder`].
    pub(in crate::checker) fn register_reference_parameter(
        &mut self,
        name: &str,
        referent: Ty,
        mutable: bool,
        binder: Option<mojito_types::origin::PointerOrigin>,
    ) {
        let Some(scope) = self.binding_scope(name) else {
            return;
        };
        let Some(owner) = self.lookup_owner(name) else {
            return;
        };
        if let Some(binder) = binder {
            self.reference_parameter_binders.insert(owner, binder);
        }
        self.reference_parameter_scopes[scope].insert(
            name.to_string(),
            mojito_types::origin::RefTy {
                referent,
                origin: mojito_types::origin::Origin::Place(mojito_types::origin::OriginPlace {
                    root: owner,
                    path: Vec::new(),
                }),
                mutability: if mutable {
                    mojito_types::origin::Mutability::Mutable
                } else {
                    mojito_types::origin::Mutability::Immutable
                },
            },
        );
    }

    /// Record the inferred mutability binder of the parameter `name` bound
    /// at `slot` (the receiver counted), when it is a `ref` parameter with
    /// no origin clause, which upstream gives an inferred
    /// `Origin[mut=m]` binder: a reference reached through it carries `m`
    /// ([`Checker::reference_actual`]).
    pub(in crate::checker) fn register_reference_binder(
        &mut self,
        name: &str,
        convention: Option<ArgConvention>,
        origin: Option<&[Expr]>,
        slot: usize,
    ) {
        if convention != Some(ArgConvention::Ref) || origin.is_some() {
            return;
        }
        let (Some(owner), Ok(slot)) = (self.lookup_owner(name), u32::try_from(slot)) else {
            return;
        };
        let binder = mojito_types::origin::Mutability::Param(
            mojito_types::origin::OriginParamId::reference_binder(slot),
        );
        self.reference_parameter_mutabilities.insert(owner, binder);
        if let Some(handle) = self
            .binding_scope(name)
            .and_then(|scope| self.reference_parameter_scopes[scope].get_mut(name))
            && handle.mutability == mojito_types::origin::Mutability::Immutable
        {
            handle.mutability = binder;
        }
    }

    /// The mutability of a reference rooted at `owner`: mutable over
    /// writable storage, a bare `ref` parameter's inferred binder
    /// ([`Self::register_reference_binder`]), immutable otherwise.
    pub(in crate::checker) fn owner_reference_mutability(
        &self,
        owner: mojito_types::origin::OwnerId,
    ) -> mojito_types::origin::Mutability {
        if self.owner_is_mutable(owner) {
            return mojito_types::origin::Mutability::Mutable;
        }
        self.reference_parameter_mutabilities
            .get(&owner)
            .copied()
            .unwrap_or(mojito_types::origin::Mutability::Immutable)
    }

    /// The enclosing struct's origin binder a `ref` parameter's clause names
    /// (`ref[Self.o] xs`, or the bare binder in a ref field annotation's
    /// position), as the pointer origin a `Pointer[T, Self.o]` field declares
    /// — the declared mutability, no interior, no subtree. `None` for a
    /// method-own binder, an inferred clause, or any other spelling.
    pub(in crate::checker) fn reference_parameter_struct_binder(
        &self,
        origin: Option<&[Expr]>,
    ) -> Option<mojito_types::origin::PointerOrigin> {
        let [expression] = origin? else {
            return None;
        };
        let name = origin_binder_name(expression)?;
        let index = self.enclosing_type_params.iter().position(|parameter| {
            parameter.name == name && parameter.bounds.as_slice() == ["Origin"]
        })?;
        if index >= self.enclosing_struct_type_params.get() {
            return None;
        }
        self.enclosing_origin_param(name).ok()
    }

    /// The struct binder recorded for a `ref` parameter binding, by name.
    pub(in crate::checker) fn lookup_reference_parameter_binder(
        &self,
        name: &str,
    ) -> Option<mojito_types::origin::PointerOrigin> {
        let owner = self.lookup_owner(name)?;
        self.reference_parameter_binders.get(&owner).cloned()
    }

    pub(in crate::checker) fn lookup_reference_parameter(
        &self,
        name: &str,
    ) -> Option<mojito_types::origin::RefTy> {
        self.reference_parameter_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    pub(in crate::checker) fn type_contains_reference(&self, ty: &Ty) -> bool {
        self.type_storage_contains(ty, false)
    }

    /// Whether storage of this type carries owner loans: reference-valued
    /// leaves, or pointers whose provenance designates checked storage.
    pub(in crate::checker) fn type_carries_loans(&self, ty: &Ty) -> bool {
        self.type_storage_contains(ty, true)
    }

    /// Whether a type MAY carry loans once populated: it carries loans
    /// already, or it contains an abstract type parameter, which may be
    /// instantiated with a loan-carrying type. Origin bookkeeping stays
    /// conservative for such storage so generic bodies record transfer
    /// effects.
    pub(in crate::checker) fn type_may_carry_loans(&self, ty: &Ty) -> bool {
        fn contains_type_parameter(ty: &Ty) -> bool {
            match ty.kind() {
                TyKind::Param { .. } => true,
                struct_ty if let Some((_, arguments)) = struct_ty.as_non_simd_struct() => {
                    arguments.iter().any(|argument| match argument {
                        mojito_types::types::TyArg::Ty(ty) => contains_type_parameter(ty),
                        _ => false,
                    })
                }
                TyKind::Ref(reference) => contains_type_parameter(&reference.referent),
                TyKind::Tuple(elements) => elements.iter().any(contains_type_parameter),
                _ => false,
            }
        }
        self.type_carries_loans(ty) || contains_type_parameter(ty) || {
            // A struct's fields, at the application's arguments, may hold
            // a type parameter the arguments leave open.
            match ty.kind() {
                struct_ty if let Some((name, arguments)) = struct_ty.as_non_simd_struct() => {
                    self.structs.get(name).is_some_and(|info| {
                        let subst = struct_subst(&info.decls, arguments);
                        info.fields
                            .iter()
                            .any(|(_, field)| contains_type_parameter(&substitute(field, &subst)))
                    })
                }
                _ => false,
            }
        }
    }

    /// The origins a value of `ty` holds loans on, and how a use of it may
    /// access them. A struct contributes its fields at its arguments, its
    /// origin parameters bound to its origin tail, a `ref` field accessed
    /// as its mutability permits: a closure's storage holds each reference
    /// capture so, over the captured origin it is applied to.
    pub(in crate::checker) fn capture_origins_in_type(
        &self,
        ty: &Ty,
    ) -> Vec<mojito_types::origin::CaptureOrigin> {
        use mojito_types::origin::{
            CaptureAccess, CaptureOrigin, CaptureOriginSet, Mutability, Origin, OriginParamId,
        };

        type Bindings = HashMap<OriginParamId, Origin>;

        fn bound(origin: Origin, origins: &Bindings) -> Origin {
            super::subst::substitute_origin_params(origin, &|id| origins.get(&id).cloned())
        }

        fn collect(checker: &Checker, ty: &Ty, origins: &Bindings, out: &mut Vec<CaptureOrigin>) {
            if let Some(element) = list_element(ty).or_else(|| set_element(ty)) {
                collect(checker, element, origins, out);
                return;
            }
            if let Some((key, value)) = dict_elements(ty) {
                collect(checker, key, origins, out);
                collect(checker, value, origins, out);
                return;
            }
            if let Some(elements) = tuple_elements(ty) {
                for element in elements {
                    collect(checker, element, origins, out);
                }
                return;
            }
            match ty.kind() {
                TyKind::Ref(reference) => {
                    out.push(CaptureOrigin::read(bound(
                        reference.origin.clone(),
                        origins,
                    )));
                }
                TyKind::Pointer { element, origin } | TyKind::LitRef { element, origin } => {
                    if let Some(origin) = origin.as_origin() {
                        out.push(CaptureOrigin::read(bound(origin, origins)));
                    }
                    collect(checker, element, origins, out);
                }
                TyKind::ComptimeList(element) => collect(checker, element, origins, out),
                TyKind::Tuple(elements)
                | TyKind::RuntimePack(elements)
                | TyKind::Variant(elements) => {
                    for element in elements {
                        collect(checker, element, origins, out);
                    }
                }
                struct_ty if let Some((name, arguments)) = struct_ty.as_non_simd_struct() => {
                    let Some(info) = checker.structs.get(name) else {
                        return;
                    };
                    let subst = struct_subst(&info.decls, arguments);
                    let own: Bindings = info
                        .tail_origin_bindings(arguments)
                        .into_iter()
                        .map(|(id, origin)| (id, bound(origin, origins)))
                        .collect();
                    for (_, field) in &info.fields {
                        let field = substitute(field, &subst);
                        match field.kind() {
                            TyKind::Ref(reference) => {
                                out.push(CaptureOrigin {
                                    origin: bound(reference.origin.clone(), &own),
                                    access: if reference.mutability == Mutability::Mutable {
                                        CaptureAccess::Write
                                    } else {
                                        CaptureAccess::Read
                                    },
                                });
                                collect(checker, &reference.referent, &own, out);
                            }
                            _ => collect(checker, &field, &own, out),
                        }
                    }
                }
                TyKind::Func(_) | TyKind::GenericFunc(_)
                    if let Some(CaptureOriginSet(captures)) =
                        ty.callable().and_then(|func| func.environment.captures()) =>
                {
                    out.extend(captures.iter().cloned());
                }
                _ => {}
            }
        }

        let mut origins = Vec::new();
        collect(self, ty, &HashMap::new(), &mut origins);
        CaptureOriginSet::concrete(origins).0
    }

    pub(in crate::checker) fn checked_capture(
        &self,
        name: &str,
        binding: mojito_types::origin::OwnerId,
        ty: Ty,
        kind: mojito_ast::ast::CaptureKind,
    ) -> mojito_checked::checked::CheckedCapture {
        use mojito_types::origin::{CaptureAccess, CaptureOrigin, Origin, OriginPlace};
        let mut origins = self.capture_origins_in_type(&ty);
        match kind {
            mojito_ast::ast::CaptureKind::Imm => origins.push(CaptureOrigin {
                origin: Origin::Place(OriginPlace {
                    root: binding,
                    path: Vec::new(),
                }),
                access: CaptureAccess::Read,
            }),
            mojito_ast::ast::CaptureKind::Mut | mojito_ast::ast::CaptureKind::Ref => {
                origins.push(CaptureOrigin {
                    origin: Origin::Place(OriginPlace {
                        root: binding,
                        path: Vec::new(),
                    }),
                    access: CaptureAccess::Write,
                });
            }
            mojito_ast::ast::CaptureKind::Copy | mojito_ast::ast::CaptureKind::Move => {}
        }
        let origins = mojito_types::origin::CaptureOriginSet::concrete(origins).0;
        mojito_checked::checked::CheckedCapture {
            name: name.to_string(),
            binding,
            ty,
            kind,
            origins,
        }
    }

    pub(in crate::checker) fn type_storage_contains(&self, ty: &Ty, pointer_loans: bool) -> bool {
        fn contains(
            checker: &Checker,
            ty: &Ty,
            pointer_loans: bool,
            seen: &mut HashSet<String>,
        ) -> bool {
            if let Some(element) = list_element(ty).or_else(|| set_element(ty)) {
                return contains(checker, element, pointer_loans, seen);
            }
            if let Some((key, value)) = dict_elements(ty) {
                return contains(checker, key, pointer_loans, seen)
                    || contains(checker, value, pointer_loans, seen);
            }
            if let Some(elements) = tuple_elements(ty) {
                return elements
                    .into_iter()
                    .any(|element| contains(checker, element, pointer_loans, seen));
            }
            match ty.kind() {
                TyKind::Ref(_) => true,
                TyKind::Pointer { element, origin } | TyKind::LitRef { element, origin } => {
                    (pointer_loans && origin.as_origin().is_some())
                        || contains(checker, element, pointer_loans, seen)
                }
                TyKind::ComptimeList(element) => contains(checker, element, pointer_loans, seen),
                TyKind::Tuple(elements) | TyKind::RuntimePack(elements) => elements
                    .iter()
                    .any(|element| contains(checker, element, pointer_loans, seen)),
                TyKind::Variant(alternatives) => alternatives
                    .iter()
                    .any(|alternative| contains(checker, alternative, pointer_loans, seen)),
                struct_ty if let Some((name, args)) = struct_ty.as_non_simd_struct() => {
                    let key = ty.to_string();
                    if !seen.insert(key.clone()) {
                        return false;
                    }
                    let result = checker.structs.get(name).is_some_and(|info| {
                        let subst = struct_subst(&info.decls, args);
                        info.fields
                            .iter()
                            .map(|(_, field)| substitute(field, &subst))
                            .any(|field| contains(checker, &field, pointer_loans, seen))
                    });
                    seen.remove(&key);
                    result
                }
                TyKind::Func(_) | TyKind::GenericFunc(_)
                    if let Some(mojito_types::origin::CaptureOriginSet(captures)) =
                        ty.callable().and_then(|func| func.environment.captures()) =>
                {
                    captures.iter().any(|capture| {
                        matches!(
                            capture.origin,
                            mojito_types::origin::Origin::Place(_)
                                | mojito_types::origin::Origin::Param(_)
                        )
                    })
                }
                _ => false,
            }
        }
        contains(self, ty, pointer_loans, &mut HashSet::new())
    }

    pub(in crate::checker) fn type_contains_unsafe_any_pointer(ty: &Ty) -> bool {
        if let Some(element) = list_element(ty).or_else(|| set_element(ty)) {
            return Self::type_contains_unsafe_any_pointer(element);
        }
        if let Some((key, value)) = dict_elements(ty) {
            return Self::type_contains_unsafe_any_pointer(key)
                || Self::type_contains_unsafe_any_pointer(value);
        }
        if let Some(elements) = tuple_elements(ty) {
            return elements
                .into_iter()
                .any(Self::type_contains_unsafe_any_pointer);
        }
        match ty.kind() {
            TyKind::Pointer {
                origin: mojito_types::origin::PointerOrigin::UnsafeAny { .. },
                ..
            } => true,
            TyKind::Pointer { element, .. } | TyKind::ComptimeList(element) => {
                Self::type_contains_unsafe_any_pointer(element)
            }
            TyKind::Tuple(elements) | TyKind::RuntimePack(elements) | TyKind::Variant(elements) => {
                elements.iter().any(Self::type_contains_unsafe_any_pointer)
            }
            _ => false,
        }
    }

    /// Origins retained by a value expression. This follows only value flow;
    /// ordinary arithmetic and reads cannot invent a stored reference handle.
    pub(in crate::checker) fn aggregate_origins(
        &self,
        expression: &Expr,
    ) -> Vec<mojito_types::origin::Origin> {
        use mojito_types::origin::Origin;

        fn append_unique(into: &mut Vec<Origin>, values: impl IntoIterator<Item = Origin>) {
            for value in values {
                if !into.contains(&value) {
                    into.push(value);
                }
            }
        }

        // A construction spelled through a type alias (`StringSlice(s)`)
        // carries what the construction it spells carries.
        if let Some(construction) = self.spelled_construction(expression) {
            return self.aggregate_origins(&construction);
        }
        if let Some(origins) = self.conversion_borrow_origins(expression) {
            return origins;
        }

        match &expression.kind {
            ExprKind::Identifier(name) => {
                let aggregate = self.lookup_aggregate_origins(name);
                if !aggregate.is_empty() {
                    return aggregate;
                }
                let found = self.lookup(name);
                match found.map(Ty::kind) {
                    Some(TyKind::Ref(reference)) => vec![reference.origin.clone()],
                    Some(TyKind::Pointer { origin, .. } | TyKind::LitRef { origin, .. }) => origin
                        .as_origin()
                        .map(|origin| vec![origin])
                        .unwrap_or_default(),
                    Some(TyKind::Func(_) | TyKind::GenericFunc(_)) => self
                        .capture_origins_in_type(found.expect("matched a callable"))
                        .into_iter()
                        .map(|capture| capture.origin)
                        .collect(),
                    _ if found.is_some_and(|ty| self.is_callable_struct(ty)) => self
                        .capture_origins_in_type(found.expect("matched a callable struct"))
                        .into_iter()
                        .map(|capture| capture.origin)
                        .collect(),
                    _ => self
                        .lookup_reference_parameter(name)
                        .map(|reference| vec![reference.origin])
                        .unwrap_or_default(),
                }
            }
            ExprKind::Member { object, field } => {
                if let Some(origins) = self.aggregate_field_origins(object).get(field) {
                    return origins.clone();
                }
                let aggregate = self.aggregate_origins(object);
                if aggregate.is_empty() {
                    self.infer_reference_value(expression)
                        .map(|reference| vec![reference.origin])
                        .unwrap_or_default()
                } else {
                    aggregate
                }
            }
            ExprKind::Transfer(inner) | ExprKind::Named { value: inner, .. } => {
                self.aggregate_origins(inner)
            }
            ExprKind::ListLit(values) | ExprKind::TupleLit(values) => {
                let mut result = Vec::new();
                for value in values {
                    append_unique(&mut result, self.aggregate_origins(value));
                }
                result
            }
            ExprKind::IfExpr {
                then_branch,
                else_branch,
                ..
            } => {
                let mut result = self.aggregate_origins(then_branch);
                append_unique(&mut result, self.aggregate_origins(else_branch));
                result
            }
            ExprKind::Call {
                name, args, kwargs, ..
            } => {
                // A view-returning call carries exactly the origins its
                // return contract bound to the call's places.
                if let Some(bound) = self.call_result_bound_origins(expression) {
                    return bound;
                }
                // A checked pointer construction retains exactly its source
                // place; the checker recorded the decision when it typed the
                // call, so shadowed `UnsafePointer` names cannot match here.
                if self
                    .node_facts
                    .borrow()
                    .get(&expression.source_span())
                    .and_then(|facts| facts.adjustment.as_deref())
                    .is_some_and(|operation| {
                        matches!(
                            operation,
                            mojito_checked::checked::SemanticAdjustment::PointerToPlace { .. }
                        )
                    })
                    && let Some(argument) = kwargs
                        .first()
                        .map(|argument| &argument.value)
                        .or_else(|| args.first())
                    && let Ok(place) = self.origin_place(argument)
                {
                    return vec![Origin::Place(place)];
                }
                if let Some(origins) = self.pointer_convert_origins(expression) {
                    return origins;
                }
                let mut result = Vec::new();
                if let Some(info) = self.structs.get(name) {
                    if info.fieldwise_init {
                        let fields: Vec<Ty> =
                            info.fields.iter().map(|(_, ty)| ty.clone()).collect();
                        for (field, argument) in fields.iter().zip(args) {
                            if matches!(field.kind(), TyKind::Ref(_)) {
                                if let Ok(reference) = self.materialized_reference_actual(argument)
                                {
                                    append_unique(&mut result, [reference.origin]);
                                }
                            } else {
                                append_unique(&mut result, self.aggregate_origins(argument));
                            }
                        }
                    } else if let Some(signatures) = info.methods.get("__init__") {
                        // Slot the call against the constructor the check
                        // selected (`StaticString(lit)` takes its
                        // `StringLiteral` by value, not `String`'s `ref`
                        // slot), else the first whose parameter shape
                        // accepts it (positional and keyword arguments alike
                        // — `Span(unsafe_ptr=…, length=…)`), so a `ref` slot
                        // contributes its reference origin and every other
                        // slot its value's aggregate origins.
                        let keyword_names: Vec<&str> =
                            kwargs.iter().map(|kw| kw.name.as_str()).collect();
                        let selected = self
                            .fact(&expression.source_span(), |facts| {
                                facts.overload_target.as_ref()
                            })
                            .and_then(|target| {
                                let instance = self.self_instance_ty(name);
                                signatures.iter().find(|signature| {
                                    super::super::overload_support::constructor_symbol(
                                        name,
                                        "__init__",
                                        signatures.len() > 1,
                                        signature,
                                        instance.as_ref(),
                                    ) == target
                                })
                            });
                        for signature in selected.into_iter().chain(signatures) {
                            let Ok(matched) = mojito_ast::call::match_call_slots(
                                &signature.names,
                                &signature.required,
                                signature.positional_only,
                                signature.keyword_only,
                                args.len(),
                                &keyword_names,
                                mojito_ast::call::CallVariadics {
                                    positional: signature.variadic.is_some(),
                                    keyword: signature.kw_variadic.is_some(),
                                },
                            ) else {
                                continue;
                            };
                            if !matched.keyword_overflow.is_empty() {
                                continue;
                            }
                            let refs = signature.ref_params.clone();
                            for (index, slot) in matched.slots.iter().enumerate() {
                                let argument = match slot {
                                    mojito_ast::call::ArgSlot::Positional(position) => {
                                        &args[*position]
                                    }
                                    mojito_ast::call::ArgSlot::Keyword(position) => {
                                        &kwargs[*position].value
                                    }
                                    mojito_ast::call::ArgSlot::Default => continue,
                                };
                                if refs.get(index).is_some_and(Option::is_some) {
                                    if let Ok(reference) =
                                        self.materialized_reference_actual(argument)
                                    {
                                        append_unique(&mut result, [reference.origin]);
                                    }
                                } else {
                                    append_unique(
                                        &mut result,
                                        self.carried_argument_origins(argument),
                                    );
                                }
                            }
                            for position in &matched.positional_overflow {
                                append_unique(
                                    &mut result,
                                    self.aggregate_origins(&args[*position]),
                                );
                            }
                            break;
                        }
                    }
                }
                if result.is_empty() {
                    for argument in args {
                        append_unique(&mut result, self.aggregate_origins(argument));
                    }
                    for argument in kwargs {
                        append_unique(&mut result, self.aggregate_origins(&argument.value));
                    }
                }
                result
            }
            // A view-typed slice result (a Span sub-slice or a StringSpan
            // keyword slice) inherits its receiver's carried origins; a
            // receiver that is itself the owning place lends that place.
            ExprKind::Slice { object, .. } | ExprKind::MultiIndex { object, .. } => {
                if matches!(
                    self.node_facts
                        .borrow()
                        .get(&expression.source_span())
                        .and_then(|facts| facts.adjustment.as_deref()),
                    Some(mojito_checked::checked::SemanticAdjustment::BorrowViewResult { .. })
                ) {
                    let carried = self.aggregate_origins(object);
                    if !carried.is_empty() {
                        return self.project_view_result_interior(expression, carried);
                    }
                    if matches!(
                        object.kind,
                        ExprKind::Identifier(_) | ExprKind::Member { .. }
                    ) && let Ok(place) = self.origin_place(object)
                    {
                        return self
                            .project_view_result_interior(expression, vec![Origin::Place(place)]);
                    }
                }
                Vec::new()
            }
            ExprKind::Invoke { args, kwargs, .. } | ExprKind::MethodCall { args, kwargs, .. } => {
                // A view-returning call carries exactly the origins its
                // return contract bound to the call's places.
                if let Some(bound) = self.call_result_bound_origins(expression) {
                    return bound;
                }
                // An `unsafe_origin_cast` result carries exactly its rebound target
                // origin, recorded by the checker when it typed the cast.
                if let Some(mojito_checked::checked::SemanticAdjustment::PointerOriginCast {
                    origin,
                    ..
                }) = self
                    .node_facts
                    .borrow()
                    .get(&expression.source_span())
                    .and_then(|facts| facts.adjustment.as_deref())
                {
                    return origin
                        .as_origin()
                        .map(|origin| vec![origin])
                        .unwrap_or_default();
                }
                if let Some(origins) = self.pointer_convert_origins(expression) {
                    return origins;
                }
                // A method whose selected contract returns an origin-bearing
                // pointer (`xs.unsafe_ptr()`) carries that rebased origin.
                if let Some(contract) = self
                    .node_facts
                    .borrow()
                    .get(&expression.source_span())
                    .and_then(|facts| facts.selected_call.as_deref())
                    && let TyKind::Pointer { origin, .. } = contract.result_ty.kind()
                    && let Some(origin) = origin.as_origin()
                {
                    return vec![origin];
                }
                // The adjustment is read out before recursing into the
                // receiver, whose inference may record adjustments of its own.
                let adjustment = self.fact(&expression.source_span(), |facts| {
                    facts.adjustment.as_deref()
                });
                // `unsafe_offset` preserves provenance: the offset pointer
                // carries whatever its receiver carried.
                if let (
                    Some(mojito_checked::checked::SemanticAdjustment::PointerOffset { .. }),
                    ExprKind::MethodCall { object, .. },
                ) = (&adjustment, &expression.kind)
                {
                    return self.aggregate_origins(object);
                }
                // A method returning a ref-field struct (a borrowing
                // view/iterator) carries its receiver's origins, exactly as a
                // view-typed slice result does, and a named or element
                // receiver place itself: the result must outlive a view
                // receiver, not only the storage the view borrows.
                if let (
                    Some(mojito_checked::checked::SemanticAdjustment::BorrowViewResult { .. }),
                    ExprKind::MethodCall { object, .. },
                ) = (&adjustment, &expression.kind)
                {
                    let mut carried = self.aggregate_origins(object);
                    if matches!(
                        object.kind,
                        ExprKind::Identifier(_) | ExprKind::Member { .. } | ExprKind::Index { .. }
                    ) && let Ok(place) = self.origin_place(object)
                    {
                        append_unique(&mut carried, [Origin::Place(place)]);
                    }
                    return self.project_view_result_interior(expression, carried);
                }
                let mut result = Vec::new();
                for argument in args {
                    append_unique(&mut result, self.aggregate_origins(argument));
                }
                for argument in kwargs {
                    append_unique(&mut result, self.aggregate_origins(&argument.value));
                }
                result
            }
            _ => Vec::new(),
        }
    }

    /// The origins a view-constructor implicit conversion's result carries,
    /// when `expression` is one.
    fn conversion_borrow_origins(
        &self,
        expression: &Expr,
    ) -> Option<Vec<mojito_types::origin::Origin>> {
        use mojito_types::origin::Origin;
        // A view-constructor implicit conversion produces a value borrowing
        // its source place: the conversion result carries that origin exactly
        // like the explicit construction's `ref [origin]` argument does, so
        // escape and staleness checks see through the implicit spelling.
        if !self.has_fact(&expression.source_span(), |facts| {
            facts.conversion_source_borrow.is_some()
        }) {
            return None;
        }
        if let Ok(place) = self.origin_place(expression) {
            return Some(vec![Origin::Place(place)]);
        }
        // A reference result lends the referent its origin names.
        if let Some(reference) = self.infer_reference_value(expression) {
            return Some(vec![reference.origin]);
        }
        // A temporary source lends its materialized anonymous owner (a
        // frame-local slot), so the view cannot escape the frame.
        if let Some(mojito_checked::checked::SemanticAdjustment::MaterializeBorrowSource {
            owner,
            ..
        }) = self
            .node_facts
            .borrow()
            .get(&expression.source_span())
            .and_then(|facts| facts.adjustment.as_deref())
        {
            return Some(vec![Origin::Place(mojito_types::origin::OriginPlace {
                root: *owner,
                path: Vec::new(),
            })]);
        }
        None
    }

    /// The origins a view-returning call's result carries when its return
    /// contract bound the returned struct's origin slots at this call
    /// (`call_result_origins`); `None` for a call that recorded nothing.
    pub(in crate::checker) fn call_result_bound_origins(
        &self,
        expression: &Expr,
    ) -> Option<Vec<mojito_types::origin::Origin>> {
        let recorded = self.call_result_origins.borrow();
        let slots = recorded.get(&expression.source_span())?;
        let mut origins = Vec::new();
        for (_, origin, _) in slots {
            if !origins.contains(origin) {
                origins.push(origin.clone());
            }
        }
        Some(origins)
    }

    /// Project a view-returning call's carried origins through the owned
    /// interior its declared return origin names (`s.rstrip()` carries
    /// `s.<bytes>`, not `s`). A call with no declared projection keeps them.
    pub(in crate::checker) fn project_view_result_interior(
        &self,
        call: &Expr,
        carried: Vec<mojito_types::origin::Origin>,
    ) -> Vec<mojito_types::origin::Origin> {
        let Some(tags) = self
            .view_result_interiors
            .borrow()
            .get(&call.source_span())
            .cloned()
        else {
            return carried;
        };
        let path: Vec<_> = tags
            .into_iter()
            .map(mojito_types::origin::OriginSeg::Interior)
            .collect();
        carried
            .into_iter()
            .map(|origin| super::project_origin(origin, &path))
            .collect()
    }

    /// The origins a `PointerConvert` result carries: a built pointer's
    /// origin, and none for a `!kgen.pointer`. `None` for any other
    /// expression.
    fn pointer_convert_origins(
        &self,
        expression: &Expr,
    ) -> Option<Vec<mojito_types::origin::Origin>> {
        match self
            .node_facts
            .borrow()
            .get(&expression.source_span())
            .and_then(|facts| facts.adjustment.as_deref())
        {
            Some(mojito_checked::checked::SemanticAdjustment::PointerConvert {
                origin, ..
            }) => Some(
                origin
                    .as_ref()
                    .and_then(mojito_types::origin::PointerOrigin::as_origin)
                    .into_iter()
                    .collect(),
            ),
            _ => None,
        }
    }
}

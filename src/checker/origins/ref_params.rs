//! Reference-parameter handles and loan-carrying type queries,
//! including capture origins.

use super::*;

impl Checker {
    /// `binder`: the enclosing struct's origin binder the parameter's clause
    /// names (`ref[Self.o] xs`), when it names one — see
    /// [`Checker::reference_parameter_struct_binder`].
    pub(in crate::checker) fn register_reference_parameter(
        &mut self,
        name: &str,
        referent: Ty,
        mutable: bool,
        binder: Option<crate::origin::PointerOrigin>,
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
            crate::origin::RefTy {
                referent: Box::new(referent),
                origin: crate::origin::Origin::Place(crate::origin::OriginPlace {
                    root: owner,
                    path: Vec::new(),
                }),
                mutability: if mutable {
                    crate::origin::Mutability::Mutable
                } else {
                    crate::origin::Mutability::Immutable
                },
            },
        );
    }

    /// The enclosing struct's origin binder a `ref` parameter's clause names
    /// (`ref[Self.o] xs`, or the bare binder in a ref field annotation's
    /// position), as the pointer origin a `Pointer[T, Self.o]` field declares
    /// — the declared mutability, no interior, no subtree. `None` for a
    /// method-own binder, an inferred clause, or any other spelling.
    pub(in crate::checker) fn reference_parameter_struct_binder(
        &self,
        origin: Option<&[Expr]>,
    ) -> Option<crate::origin::PointerOrigin> {
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
    ) -> Option<crate::origin::PointerOrigin> {
        let owner = self.lookup_owner(name)?;
        self.reference_parameter_binders.get(&owner).cloned()
    }

    pub(in crate::checker) fn lookup_reference_parameter(
        &self,
        name: &str,
    ) -> Option<crate::origin::RefTy> {
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
    /// already, or it contains a capturing callable whose declared
    /// environment has not yet been narrowed to a concrete capture set
    /// (`capturing[_]` / `capturing[origins]` storage). Origin bookkeeping
    /// for such storage must record the populating value's concrete capture
    /// origins even though the declared type alone is loan-blind.
    pub(in crate::checker) fn type_may_carry_loans(&self, ty: &Ty) -> bool {
        fn contains_open_captures(ty: &Ty) -> bool {
            use crate::origin::{CallableEnvironment, CaptureOriginSet};
            match ty {
                // An abstract type parameter may be instantiated with a
                // loan-carrying type: origin bookkeeping stays conservative
                // so generic bodies record transfer effects.
                Ty::Param { .. } => true,
                Ty::Func { environment, .. } => matches!(
                    environment,
                    CallableEnvironment::Capturing(
                        CaptureOriginSet::Infer | CaptureOriginSet::Param(_)
                    )
                ),
                Ty::Struct(_, arguments) => arguments.iter().any(|argument| match argument {
                    crate::types::TyArg::Ty(ty) => contains_open_captures(ty),
                    _ => false,
                }),
                Ty::Ref(reference) => contains_open_captures(&reference.referent),
                Ty::Tuple(elements) => elements.iter().any(contains_open_captures),
                _ => false,
            }
        }
        self.type_carries_loans(ty) || contains_open_captures(ty) || {
            // A struct's declared fields may hold open-capture callables.
            match ty {
                Ty::Struct(name, _) => self.structs.get(name).is_some_and(|info| {
                    info.fields
                        .iter()
                        .any(|(_, field)| contains_open_captures(field))
                }),
                _ => false,
            }
        }
    }

    pub(in crate::checker) fn capture_origins_in_type(
        &self,
        ty: &Ty,
    ) -> Vec<crate::origin::CaptureOrigin> {
        use crate::origin::{CallableEnvironment, CaptureOrigin, CaptureOriginSet};

        fn collect(checker: &Checker, ty: &Ty, out: &mut Vec<CaptureOrigin>) {
            if let Some(element) = list_element(ty).or_else(|| set_element(ty)) {
                collect(checker, element, out);
                return;
            }
            if let Some((key, value)) = dict_elements(ty) {
                collect(checker, key, out);
                collect(checker, value, out);
                return;
            }
            if let Some(elements) = tuple_elements(ty) {
                for element in elements {
                    collect(checker, element, out);
                }
                return;
            }
            match ty {
                Ty::Ref(reference) => out.push(CaptureOrigin::read(reference.origin.clone())),
                Ty::Pointer { element, origin } => {
                    if let Some(origin) = origin.as_origin() {
                        out.push(CaptureOrigin::read(origin));
                    }
                    collect(checker, element, out);
                }
                Ty::ComptimeList(element) => collect(checker, element, out),
                Ty::Tuple(elements) | Ty::RuntimePack(elements) | Ty::Variant(elements) => {
                    for element in elements {
                        collect(checker, element, out);
                    }
                }
                Ty::Struct(name, arguments) => {
                    let Some(info) = checker.structs.get(name) else {
                        return;
                    };
                    let subst = struct_subst(&info.decls, arguments);
                    for (_, field) in &info.fields {
                        collect(checker, &substitute(field, &subst), out);
                    }
                }
                Ty::Func {
                    environment:
                        CallableEnvironment::Capturing(CaptureOriginSet::Concrete(captures)),
                    ..
                }
                | Ty::GenericFunc {
                    environment:
                        CallableEnvironment::Capturing(CaptureOriginSet::Concrete(captures)),
                    ..
                } => out.extend(captures.iter().cloned()),
                _ => {}
            }
        }

        let mut origins = Vec::new();
        collect(self, ty, &mut origins);
        let CaptureOriginSet::Concrete(origins) = CaptureOriginSet::concrete(origins) else {
            unreachable!("concrete capture canonicalization stays concrete")
        };
        origins
    }

    pub(in crate::checker) fn checked_capture(
        &self,
        name: &str,
        binding: crate::origin::OwnerId,
        ty: Ty,
        kind: crate::ast::CaptureKind,
    ) -> crate::checked::CheckedCapture {
        use crate::origin::{CaptureAccess, CaptureOrigin, Origin, OriginPlace};
        let mut origins = self.capture_origins_in_type(&ty);
        match kind {
            crate::ast::CaptureKind::Imm => origins.push(CaptureOrigin {
                origin: Origin::Place(OriginPlace {
                    root: binding,
                    path: Vec::new(),
                }),
                access: CaptureAccess::Read,
            }),
            crate::ast::CaptureKind::Mut | crate::ast::CaptureKind::Ref => {
                origins.push(CaptureOrigin {
                    origin: Origin::Place(OriginPlace {
                        root: binding,
                        path: Vec::new(),
                    }),
                    access: CaptureAccess::Write,
                })
            }
            crate::ast::CaptureKind::Copy | crate::ast::CaptureKind::Move => {}
        }
        let crate::origin::CaptureOriginSet::Concrete(origins) =
            crate::origin::CaptureOriginSet::concrete(origins)
        else {
            unreachable!("concrete capture canonicalization stays concrete")
        };
        crate::checked::CheckedCapture {
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
            match ty {
                Ty::Ref(_) => true,
                Ty::Pointer { element, origin } => {
                    (pointer_loans && origin.as_origin().is_some())
                        || contains(checker, element, pointer_loans, seen)
                }
                Ty::ComptimeList(element) => contains(checker, element, pointer_loans, seen),
                Ty::Tuple(elements) | Ty::RuntimePack(elements) => elements
                    .iter()
                    .any(|element| contains(checker, element, pointer_loans, seen)),
                Ty::Variant(alternatives) => alternatives
                    .iter()
                    .any(|alternative| contains(checker, alternative, pointer_loans, seen)),
                Ty::Struct(name, args) => {
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
                Ty::Func {
                    environment:
                        crate::origin::CallableEnvironment::Capturing(
                            crate::origin::CaptureOriginSet::Concrete(captures),
                        ),
                    ..
                }
                | Ty::GenericFunc {
                    environment:
                        crate::origin::CallableEnvironment::Capturing(
                            crate::origin::CaptureOriginSet::Concrete(captures),
                        ),
                    ..
                } => captures.iter().any(|capture| {
                    matches!(
                        capture.origin,
                        crate::origin::Origin::Place(_) | crate::origin::Origin::Param(_)
                    )
                }),
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
        match ty {
            Ty::Pointer {
                origin: crate::origin::PointerOrigin::UnsafeAny { .. },
                ..
            } => true,
            Ty::Pointer { element, .. } | Ty::ComptimeList(element) => {
                Self::type_contains_unsafe_any_pointer(element)
            }
            Ty::Tuple(elements) | Ty::RuntimePack(elements) | Ty::Variant(elements) => {
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
    ) -> Vec<crate::origin::Origin> {
        use crate::origin::Origin;

        fn append_unique(into: &mut Vec<Origin>, values: impl IntoIterator<Item = Origin>) {
            for value in values {
                if !into.contains(&value) {
                    into.push(value);
                }
            }
        }

        // A view-constructor implicit conversion produces a value borrowing
        // its source place: the conversion result carries that origin exactly
        // like the explicit construction's `ref [origin]` argument does, so
        // escape and staleness checks see through the implicit spelling.
        if self
            .conversion_source_borrows
            .borrow()
            .contains_key(&expression.source_span())
            && let Ok(place) = self.origin_place(expression)
        {
            return vec![Origin::Place(place)];
        }

        match &expression.kind {
            ExprKind::Identifier(name) => {
                let aggregate = self.lookup_aggregate_origins(name);
                if !aggregate.is_empty() {
                    return aggregate;
                }
                match self.lookup(name) {
                    Some(Ty::Ref(reference)) => vec![reference.origin.clone()],
                    Some(Ty::Pointer { origin, .. }) => origin
                        .as_origin()
                        .map(|origin| vec![origin])
                        .unwrap_or_default(),
                    Some(ty @ (Ty::Func { .. } | Ty::GenericFunc { .. })) => self
                        .capture_origins_in_type(ty)
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
                if !aggregate.is_empty() {
                    aggregate
                } else {
                    self.infer_reference_value(expression)
                        .map(|reference| vec![reference.origin])
                        .unwrap_or_default()
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
                // A checked pointer construction retains exactly its source
                // place; the checker recorded the decision when it typed the
                // call, so shadowed `UnsafePointer` names cannot match here.
                if self
                    .operation_adjustments
                    .borrow()
                    .get(&expression.source_span())
                    .is_some_and(|operation| {
                        matches!(
                            operation,
                            crate::checked::SemanticAdjustment::PointerToPlace { .. }
                        )
                    })
                    && let Some(argument) = kwargs.first()
                    && let Ok(place) = self.origin_place(&argument.value)
                {
                    return vec![Origin::Place(place)];
                }
                let mut result = Vec::new();
                if let Some(info) = self.structs.get(name) {
                    if info.fieldwise_init {
                        let fields: Vec<Ty> =
                            info.fields.iter().map(|(_, ty)| ty.clone()).collect();
                        for (field, argument) in fields.iter().zip(args) {
                            if matches!(field, Ty::Ref(_)) {
                                if let Ok(reference) = self.materialized_reference_actual(argument)
                                {
                                    append_unique(&mut result, [reference.origin]);
                                }
                            } else {
                                append_unique(&mut result, self.aggregate_origins(argument));
                            }
                        }
                    } else if let Some(signature) =
                        info.methods.get("__init__").and_then(|signatures| {
                            signatures.iter().find(|sig| sig.params.len() == args.len())
                        })
                    {
                        let refs = signature.ref_params.clone();
                        for (index, argument) in args.iter().enumerate() {
                            if refs.get(index).is_some_and(Option::is_some) {
                                if let Ok(reference) = self.materialized_reference_actual(argument)
                                {
                                    append_unique(&mut result, [reference.origin]);
                                }
                            } else {
                                append_unique(&mut result, self.aggregate_origins(argument));
                            }
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
                    self.operation_adjustments
                        .borrow()
                        .get(&expression.source_span()),
                    Some(crate::checked::SemanticAdjustment::BorrowViewResult)
                ) {
                    let carried = self.aggregate_origins(object);
                    if !carried.is_empty() {
                        return carried;
                    }
                    if matches!(
                        object.kind,
                        ExprKind::Identifier(_) | ExprKind::Member { .. }
                    ) && let Ok(place) = self.origin_place(object)
                    {
                        return vec![Origin::Place(place)];
                    }
                }
                Vec::new()
            }
            ExprKind::Invoke { args, kwargs, .. } | ExprKind::MethodCall { args, kwargs, .. } => {
                // An `unsafe_origin_cast` result carries exactly its rebound target
                // origin, recorded by the checker when it typed the cast.
                if let Some(crate::checked::SemanticAdjustment::PointerOriginCast { origin }) = self
                    .operation_adjustments
                    .borrow()
                    .get(&expression.source_span())
                {
                    return origin
                        .as_origin()
                        .map(|origin| vec![origin])
                        .unwrap_or_default();
                }
                // A method whose selected contract returns an origin-bearing
                // pointer (`xs.unsafe_ptr()`) carries that rebased origin.
                if let Some(contract) = self.selected_calls.borrow().get(&expression.source_span())
                    && let Ty::Pointer { origin, .. } = &contract.result_ty
                    && let Some(origin) = origin.as_origin()
                {
                    return vec![origin];
                }
                // `unsafe_offset` preserves provenance: the offset pointer
                // carries whatever its receiver carried.
                if let (
                    Some(crate::checked::SemanticAdjustment::PointerOffset),
                    ExprKind::MethodCall { object, .. },
                ) = (
                    self.operation_adjustments
                        .borrow()
                        .get(&expression.source_span()),
                    &expression.kind,
                ) {
                    return self.aggregate_origins(object);
                }
                // A method returning a ref-field struct (a borrowing
                // view/iterator) carries its receiver's origins, exactly as a
                // view-typed slice result does.
                if let (
                    Some(crate::checked::SemanticAdjustment::BorrowViewResult),
                    ExprKind::MethodCall { object, .. },
                ) = (
                    self.operation_adjustments
                        .borrow()
                        .get(&expression.source_span()),
                    &expression.kind,
                ) {
                    let carried = self.aggregate_origins(object);
                    if !carried.is_empty() {
                        return carried;
                    }
                    if matches!(
                        object.kind,
                        ExprKind::Identifier(_) | ExprKind::Member { .. }
                    ) && let Ok(place) = self.origin_place(object)
                    {
                        return vec![Origin::Place(place)];
                    }
                    return Vec::new();
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
}

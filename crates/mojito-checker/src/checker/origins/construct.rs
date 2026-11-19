//! Constructor origin binding: the struct origin binders a constructor's
//! parameters name — a `Pointer[Self.T, Self.origin]` parameter (upstream
//! `Span(unsafe_ptr=, length=)`), a `ref [Self.origin]` parameter, a
//! struct-typed parameter carrying `Self.origin` in its own tail
//! (`RefBox[Self.origin]`), or a `@fieldwise_init` field of any of those
//! shapes — bound from the call's arguments, checked against an explicitly
//! applied origin (`Span[Byte, origin_of(self)](...)`), and written into the
//! constructed value's origin tail.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::origin::{Mutability, Origin, OriginParamId, PointerOrigin};
use mojito_types::types::TyKind;

/// The struct origin binders one construction binds: pointer provenance for
/// substitution into pointer-typed parameters, and the origin every bound
/// binder names for the constructed value's tail.
#[derive(Default)]
pub(in crate::checker) struct ConstructorOriginBindings {
    pub(in crate::checker) pointers: HashMap<OriginParamId, PointerOrigin>,
    pub(in crate::checker) origins: HashMap<OriginParamId, Origin>,
    /// The binders this binding binds.
    binders: CalleeBinders,
}

/// The origin binders a callee's call binds from its arguments: every one
/// its parameters name (a function value's contract), or exactly those a
/// declared list declares, a method's numbered past its struct's, which its
/// receiver or construction binds instead.
#[derive(Clone, Default)]
pub(in crate::checker) enum CalleeBinders {
    #[default]
    Every,
    Declared(Vec<OriginParamId>),
}

impl CalleeBinders {
    /// Whether the call binds the binder `id`.
    pub(in crate::checker) fn binds(&self, id: OriginParamId) -> bool {
        match self {
            Self::Every => true,
            Self::Declared(declared) => declared.contains(&id),
        }
    }
}

/// The origin binders `decls`, a callee's whole declared list, declares
/// for its call to bind from the arguments' places: every `Origin` binder,
/// an `OriginSet` left alone.
pub(in crate::checker) fn declared_origin_binders(
    decls: &[ParamDecl],
) -> impl Iterator<Item = OriginParamId> + '_ {
    decls.iter().filter_map(|decl| match decl {
        ParamDecl::Origin {
            origin, mutability, ..
        } if *mutability != mojito_types::types::OriginMutability::Set => Some(*origin),
        _ => None,
    })
}

impl ConstructorOriginBindings {
    /// Substitute the bound binders into a parameter or field type: pointer
    /// binders through their provenance, struct tails through their origin.
    pub(in crate::checker) fn substitute(&self, ty: &Ty) -> Ty {
        substitute_struct_origin_tails(
            &substitute_pointer_origin_params(ty, &self.pointers),
            &self.origins,
        )
    }

    /// [`Self::substitute`] for a declared result: a binder this call bound
    /// only through a struct argument's origin tail (`v: View[o]` over
    /// `-> Pointer[T, o]`) rebinds the result's pointer provenances too,
    /// taking a parametric binder's mutability from `binder_mutability`
    /// ([`bind_pointer_origin_tails`]).
    /// The tail binding goes in first, so a caller binder the pointer
    /// bindings substitute is never captured.
    pub(in crate::checker) fn substitute_result(
        &self,
        ty: &Ty,
        binder_mutability: &dyn Fn(OriginParamId, &Origin) -> Option<Mutability>,
    ) -> Ty {
        let tails: HashMap<OriginParamId, Origin> = self
            .origins
            .iter()
            .filter(|(id, _)| !self.pointers.contains_key(id))
            .map(|(id, origin)| (*id, origin.clone()))
            .collect();
        self.substitute(&bind_pointer_origin_tails(
            ty,
            &tails,
            Some(binder_mutability),
        ))
    }

    /// [`Self::substitute`] over a parameter list.
    pub(in crate::checker) fn substitute_all(&self, tys: &[Ty]) -> Vec<Ty> {
        tys.iter().map(|ty| self.substitute(ty)).collect()
    }

    /// Whether the call bound no binder.
    pub(in crate::checker) fn is_empty(&self) -> bool {
        self.pointers.is_empty() && self.origins.is_empty()
    }

    /// Substitute the bound binders into a `ref` result contract: each
    /// `SigOrigin::Bound` naming a solved binder becomes the origin the call
    /// bound it to, as the declaration's result is its signature with every
    /// inferred parameter substituted. A result over exactly one
    /// mut-parametric binder takes that binding's mutability, as upstream
    /// infers the binder's `mut` with its origin: a pointer's from its
    /// provenance, a struct tail's from its owner (`owner_mutability`).
    pub(in crate::checker) fn substitute_ref_sig(
        &self,
        signature: &mojito_types::origin::RefSig,
        owner_mutability: &dyn Fn(mojito_types::origin::OwnerId) -> Mutability,
    ) -> mojito_types::origin::RefSig {
        use mojito_types::origin::{SigMutability, SigOrigin};
        let mutability = match (&signature.mutability, &signature.origin) {
            (
                SigMutability::Infer | SigMutability::BoolParam(_),
                SigOrigin::Bound(Origin::Param(id)),
            ) => match self.bound_mutability(*id, owner_mutability) {
                Some(Mutability::Mutable) => SigMutability::Mutable,
                Some(Mutability::Immutable) => SigMutability::Immutable,
                Some(Mutability::Param(_)) | None => signature.mutability.clone(),
            },
            _ => signature.mutability.clone(),
        };
        mojito_types::origin::RefSig {
            origin: self.substitute_sig_origin(&signature.origin),
            mutability,
        }
    }

    /// The constructed value's origin tail: each slot's explicit origin where
    /// the application supplied one, else the origin its binder bound, else
    /// unbound.
    pub(in crate::checker) fn bind_tail(
        &self,
        slots: &[(OriginParamId, &mojito_ast::ast::TypeParam)],
        tail: &[Origin],
    ) -> Vec<Origin> {
        slots
            .iter()
            .zip(tail)
            .map(|((id, _), explicit)| match explicit {
                Origin::Unbound => self.origins.get(id).cloned().unwrap_or(Origin::Unbound),
                bound => bound.clone(),
            })
            .collect()
    }

    /// The mutability binder `id` was bound with: a pointer's provenance,
    /// else the storage its origin names.
    pub(in crate::checker) fn bound_mutability(
        &self,
        id: OriginParamId,
        owner_mutability: &dyn Fn(mojito_types::origin::OwnerId) -> Mutability,
    ) -> Option<Mutability> {
        if let Some(provenance) = self.pointers.get(&id) {
            return match (provenance.statically_mutable(), provenance) {
                (Some(true), _) => Some(Mutability::Mutable),
                (Some(false), _) => Some(Mutability::Immutable),
                (
                    None,
                    PointerOrigin::Param { mutability, .. }
                    | PointerOrigin::SelfPlace { mutability, .. },
                ) => Some(*mutability),
                (None, _) => None,
            };
        }
        origin_mutability(self.origins.get(&id)?, owner_mutability)
    }

    fn substitute_sig_origin(
        &self,
        origin: &mojito_types::origin::SigOrigin,
    ) -> mojito_types::origin::SigOrigin {
        use mojito_types::origin::SigOrigin;
        match origin {
            SigOrigin::Bound(bound) => {
                SigOrigin::Bound(super::substitute_origin_params(bound.clone(), &|id| {
                    self.origins.get(&id).cloned()
                }))
            }
            SigOrigin::Projected(base, path) => {
                SigOrigin::Projected(Box::new(self.substitute_sig_origin(base)), path.clone())
            }
            SigOrigin::Union(members) => SigOrigin::Union(
                members
                    .iter()
                    .map(|member| self.substitute_sig_origin(member))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    fn bind_pointer(
        &mut self,
        id: OriginParamId,
        provenance: &PointerOrigin,
        conflict: impl Fn() -> TypeError,
    ) -> Result<(), TypeError> {
        if !self.binders.binds(id) {
            return Ok(());
        }
        match self.pointers.get(&id) {
            None => {
                self.pointers.insert(id, provenance.clone());
                if let Some(origin) = provenance
                    .as_origin()
                    .or_else(|| untracked_pointer_origin(provenance))
                {
                    self.origins
                        .entry(id)
                        .or_insert_with(|| tail_origin(origin));
                }
                Ok(())
            }
            Some(existing) if existing == provenance => Ok(()),
            Some(_) => Err(conflict()),
        }
    }

    /// Bind a tail slot; a second binding of the same slot must name the
    /// same storage (a subtree projection does not make it different).
    fn bind_origin(
        &mut self,
        id: OriginParamId,
        origin: &Origin,
        conflict: impl Fn() -> TypeError,
    ) -> Result<(), TypeError> {
        if matches!(origin, Origin::Unbound) || !self.binders.binds(id) {
            return Ok(());
        }
        let origin = tail_origin(origin.clone());
        match self.origins.get(&id) {
            None => {
                self.origins.insert(id, origin);
                Ok(())
            }
            Some(existing) if origin.coerces_to(existing) => Ok(()),
            Some(_) => Err(conflict()),
        }
    }
}

impl Checker {
    /// A call's solved arguments over its callee's whole declared list
    /// `decls`, as upstream's call holds a value for every declared
    /// parameter: `arguments`, the type and value binders' solutions over
    /// the list's argument view ([`ParamDecl::argument_view`]), with each
    /// origin binder's origin in place, the one its bracket bound
    /// (`bracket`) or else the one the arguments' places bound it to
    /// (`bindings`), and each `mut=` binder's value, that origin's
    /// mutability. A mutability the caller's own binder decides is
    /// [`CtValue::Deferred`], as its value arrives with the caller's
    /// instance. `arguments` is returned as it stands when it is no
    /// solution of the argument view.
    pub(in crate::checker) fn declared_call_arguments(
        &self,
        decls: &[ParamDecl],
        arguments: &[TyArg],
        bracket: &HashMap<OriginParamId, Origin>,
        bindings: Option<&ConstructorOriginBindings>,
    ) -> Vec<TyArg> {
        if arguments.len() != ParamDecl::argument_binders(decls).count()
            || arguments.len() == decls.len()
        {
            return arguments.to_vec();
        }
        let origin = |id: OriginParamId| {
            bracket
                .get(&id)
                .or_else(|| bindings.and_then(|bindings| bindings.origins.get(&id)))
                .cloned()
        };
        let mutability = |id: OriginParamId| match bracket.get(&id) {
            Some(origin) => self.bound_origin_mutability(origin),
            None => bindings?.bound_mutability(id, &|owner| self.owner_reference_mutability(owner)),
        };
        let mut solved = arguments.iter();
        decls
            .iter()
            .map(|decl| match decl {
                ParamDecl::Origin { origin: id, .. } => {
                    TyArg::Origin(origin(*id).unwrap_or(Origin::Unbound))
                }
                _ if decl.is_origin_mutability_binder(decls) => {
                    let governed = decls.iter().find_map(|candidate| match candidate {
                        ParamDecl::Origin {
                            origin,
                            mutability: mojito_types::types::OriginMutability::Binder(binder),
                            ..
                        } if binder.id == *decl.id() => Some(*origin),
                        _ => None,
                    });
                    TyArg::Val(match governed.and_then(mutability) {
                        Some(Mutability::Mutable) => CtValue::Bool(true),
                        Some(Mutability::Immutable) => CtValue::Bool(false),
                        Some(Mutability::Param(_)) | None => CtValue::Deferred(decl.binder()),
                    })
                }
                _ => solved
                    .next()
                    .cloned()
                    .unwrap_or(TyArg::Origin(Origin::Unbound)),
            })
            .collect()
    }

    /// The checked type of a construction: the solved binder prefix followed
    /// by the origin tail the application and the arguments bound.
    pub(in crate::checker) fn constructed_type(
        &self,
        name: &str,
        prefix: Vec<TyArg>,
        tail: &[Origin],
        bindings: &ConstructorOriginBindings,
    ) -> Ty {
        let mut arguments = prefix;
        if let Some(info) = self.structs.get(name) {
            arguments.extend(
                bindings
                    .bind_tail(&info.origin_slots(), tail)
                    .into_iter()
                    .map(TyArg::Origin),
            );
        }
        Self::struct_instance_type(name, arguments)
    }

    /// Bind the struct origin binders the selected constructor's parameters
    /// name from the arguments filling those slots, and check every slot
    /// naming an explicitly applied origin against its argument.
    ///
    /// `bound` pairs each argument expression with the parameter index it
    /// fills and that parameter's declared (unsubstituted) type; `arg_tys`
    /// runs parallel to it. The result maps each struct binder to what the
    /// call binds it to, for substitution into the parameter types before
    /// coercion ([`ConstructorOriginBindings::substitute`]) and for the
    /// constructed value's tail.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::checker) fn bind_constructor_origins(
        &self,
        struct_name: &str,
        callee: &str,
        source_params: &[mojito_ast::ast::TypeParam],
        sig: &MethodSig,
        bound: &[(usize, &Expr, &Ty)],
        arg_tys: &[Ty],
        explicit: &[super::super::type_resolution::ExplicitStructOrigin],
    ) -> Result<ConstructorOriginBindings, TypeError> {
        let mut bindings = ConstructorOriginBindings::default();
        for ((index, expression, pattern), actual) in bound.iter().zip(arg_tys) {
            let parameter = sig.names.get(*index).map_or("?", String::as_str);
            let context = || format!("argument '{parameter}' to '{struct_name}.{callee}'");
            Self::bind_origin_pattern(
                struct_name,
                &context,
                source_params,
                pattern,
                actual,
                explicit,
                &mut bindings,
            )?;
            // A `ref [Self.o]` parameter binds its binder to the argument's
            // origin: a reference-valued place (a `ref [o]` parameter of the
            // enclosing signature) carries its declared origin, any other
            // place its own.
            if let Some(Some(PointerOrigin::Param { id, .. })) = sig.origin_binders.get(*index)
                && (id.0 as usize) < source_params.len()
                && let Some(actual) = self.reference_argument_origin(expression)
            {
                if let Some(explicit) = explicit.iter().find(|origin| origin.id == *id)
                    && !origin_is_within(&actual, &explicit.origin)
                {
                    let slot = &source_params[id.0 as usize];
                    return Err(TypeError::TypeMismatch {
                        expected: format!(
                            "a reference whose origin lies within the supplied '{}' argument",
                            slot.name
                        ),
                        found: format!("a reference of origin {actual:?}"),
                        context: context(),
                    });
                }
                bindings.bind_origin(*id, &actual, || {
                    conflicting_binding(struct_name, &source_params[id.0 as usize].name)
                })?;
            }
        }
        self.bind_refined_origins(struct_name, sig, explicit, &mut bindings)?;
        Ok(bindings)
    }

    /// Bind the struct origin slots an initializer's `out self` refines to
    /// a concrete origin (`out self: StaticString` on `StringSpan[origin]`
    /// binds `origin` to the static origin). An explicitly applied origin
    /// naming different storage conflicts.
    pub(in crate::checker) fn bind_refined_origins(
        &self,
        struct_name: &str,
        sig: &MethodSig,
        explicit: &[super::super::type_resolution::ExplicitStructOrigin],
        bindings: &mut ConstructorOriginBindings,
    ) -> Result<(), TypeError> {
        let (Some(refined), Some(instance)) =
            (&sig.refined_self, self.self_instance_ty(struct_name))
        else {
            return Ok(());
        };
        let arguments = |ty: &Ty| {
            mojito_types::types::nominal_parts(ty)
                .map(|(_, arguments)| arguments.into_owned())
                .unwrap_or_default()
        };
        for (defined, binder) in arguments(refined).iter().zip(&arguments(&instance)) {
            if let (TyArg::Origin(origin), TyArg::Origin(Origin::Param(id))) = (defined, binder)
                && !matches!(origin, Origin::Param(_) | Origin::SelfParam)
            {
                // The constructor's own binders the arguments solved
                // (`out self: V[Self.T, ImmOrigin(src)]`) are the call's.
                let origin = &super::substitute_origin_params(origin.clone(), &|own| {
                    bindings.origins.get(&own).cloned()
                });
                let conflict = || {
                    let slot = self
                        .structs
                        .get(struct_name)
                        .and_then(|info| info.source_params.get(id.0 as usize))
                        .map_or("?", |slot| slot.name.as_str());
                    conflicting_binding(struct_name, slot)
                };
                if explicit
                    .iter()
                    .any(|applied| applied.id == *id && !origin.coerces_to(&applied.origin))
                {
                    return Err(conflict());
                }
                bindings.bind_origin(*id, origin, conflict)?;
            }
        }
        Ok(())
    }

    /// Bind the struct origin binders a `@fieldwise_init` construction's
    /// field types name from the arguments filling those fields, checking
    /// them against the explicitly applied origins like a hand-written
    /// constructor's parameters.
    pub(in crate::checker) fn bind_fieldwise_origins(
        &self,
        struct_name: &str,
        source_params: &[mojito_ast::ast::TypeParam],
        fields: &[(String, Ty)],
        args: &[Expr],
        arg_tys: &[Ty],
        explicit: &[super::super::type_resolution::ExplicitStructOrigin],
    ) -> Result<ConstructorOriginBindings, TypeError> {
        let mut bindings = ConstructorOriginBindings::default();
        for (((field, pattern), expression), actual) in fields.iter().zip(args).zip(arg_tys) {
            let context = || format!("field '{field}' of '{struct_name}'");
            // A reference field binds from the argument's origin when the
            // argument's own type carries no reference.
            let reference_actual;
            let actual = match (pattern.kind(), actual.kind()) {
                (TyKind::Ref(_), TyKind::Ref(_)) => actual,
                (TyKind::Ref(declared), _) => match self.reference_argument_origin(expression) {
                    Some(origin) => {
                        reference_actual = Ty::new(TyKind::Ref(mojito_types::origin::RefTy {
                            referent: declared.referent.clone(),
                            origin,
                            mutability: declared.mutability,
                        }));
                        &reference_actual
                    }
                    None => actual,
                },
                _ => actual,
            };
            Self::bind_origin_pattern(
                struct_name,
                &context,
                source_params,
                pattern,
                actual,
                explicit,
                &mut bindings,
            )?;
        }
        Ok(bindings)
    }

    /// Bind a callable's own origin binders from the arguments filling the
    /// parameters that name them: pointer binders through
    /// [`Self::bind_callable_pointer_origins`], and struct origin tails
    /// (`def stash[o: Origin](mut sink: List[RefBox[o]], var box: RefBox[o])`
    /// called with a `List[RefBox[origin_of(view)]]`) from the actuals' tails.
    /// `bound` pairs a parameter index with the actual type.
    pub(in crate::checker) fn bind_callee_origins(
        callee: &str,
        names: &[String],
        params: &[Ty],
        bound: &[(usize, &Ty)],
        binders: CalleeBinders,
    ) -> Result<ConstructorOriginBindings, TypeError> {
        let mut bindings = ConstructorOriginBindings {
            pointers: Self::bind_callable_pointer_origins(callee, names, params, bound, &binders)?,
            origins: HashMap::new(),
            binders,
        };
        for (id, provenance) in &bindings.pointers {
            if let Some(origin) = provenance
                .as_origin()
                .or_else(|| untracked_pointer_origin(provenance))
            {
                bindings.origins.insert(*id, origin);
            }
        }
        for (index, actual) in bound {
            let Some(pattern) = params.get(*index) else {
                continue;
            };
            let parameter = names.get(*index).map_or("?", String::as_str);
            bind_callable_tail_pattern(pattern, actual, &mut bindings, &|| TypeError::BadCall {
                func: callee.to_string(),
                reason: format!("arguments bind conflicting origins for parameter '{parameter}'"),
            })?;
        }
        Ok(bindings)
    }

    /// Bind a callable's own origin binders named by its pointer-typed
    /// parameters from the arguments filling those slots
    /// (`def peek[o: Origin](p: Pointer[Int, o])` called with `Pointer(to=x)`
    /// binds `o` to `x`'s provenance). A `MutOrigin` binder rejects an
    /// immutable-origin actual; a projected binder is checked by coercion,
    /// not bound. `bound` pairs a parameter index with the actual type; a
    /// binder `binders` does not bind is an enclosing struct's, left alone.
    pub(in crate::checker) fn bind_callable_pointer_origins(
        callee: &str,
        names: &[String],
        params: &[Ty],
        bound: &[(usize, &Ty)],
        binders: &CalleeBinders,
    ) -> Result<HashMap<OriginParamId, PointerOrigin>, TypeError> {
        let mut bindings: HashMap<OriginParamId, PointerOrigin> = HashMap::new();
        for (index, actual) in bound {
            let (
                Some((
                    _,
                    PointerOrigin::Param {
                        id,
                        mutability,
                        interior,
                        subtree,
                    },
                )),
                Some((_, actual_origin)),
            ) = (
                params
                    .get(*index)
                    .and_then(|param| param.kind().pointer_or_lit_ref()),
                actual.kind().pointer_or_lit_ref(),
            )
            else {
                continue;
            };
            if !binders.binds(id) {
                continue;
            }
            let parameter = names.get(*index).map_or("?", String::as_str);
            if mutability == Mutability::Mutable
                && actual_origin.statically_mutable() == Some(false)
            {
                return Err(TypeError::TypeMismatch {
                    expected: format!(
                        "a mutable-origin pointer for parameter '{parameter}' of '{callee}'"
                    ),
                    found: "an immutable-origin pointer".to_string(),
                    context: format!("argument '{parameter}' to '{callee}'"),
                });
            }
            if !interior.is_empty() || subtree {
                continue;
            }
            match bindings.get(&id) {
                None => {
                    bindings.insert(id, actual_origin);
                }
                Some(existing) if *existing == actual_origin => {}
                Some(_) => {
                    return Err(TypeError::BadCall {
                        func: callee.to_string(),
                        reason: format!(
                            "arguments bind conflicting origins for parameter '{parameter}'"
                        ),
                    });
                }
            }
        }
        Ok(bindings)
    }

    /// The origin an argument lends to a `ref [o]` parameter or reference
    /// field: a reference-valued place (a `ref [o]` parameter of the enclosing
    /// signature, a `ref` binding) carries its declared origin, any other
    /// place its own storage, and a temporary the hidden slot it materializes
    /// into.
    fn reference_argument_origin(&self, expression: &Expr) -> Option<Origin> {
        // A `ref [o]` parameter of the enclosing signature lends its binder.
        if let ExprKind::Identifier(name) = &expression.kind
            && let Some(PointerOrigin::Param { id, .. }) =
                self.lookup_reference_parameter_binder(name)
        {
            return Some(Origin::Param(id));
        }
        self.returned_reference_parameter_origin(expression)
            .or_else(|| {
                self.materialized_reference_actual(expression)
                    .ok()
                    .map(|reference| reference.origin)
            })
    }

    /// Bind the struct binders one declared parameter or field type names
    /// from the actual filling it: a pointer's binder from the pointer's
    /// provenance, a reference's binder from the reference's origin, and a
    /// struct-typed pattern's tail binders from the actual's tail (recursing
    /// through nested type arguments and tuples).
    #[allow(clippy::too_many_arguments)]
    fn bind_origin_pattern(
        struct_name: &str,
        context: &dyn Fn() -> String,
        source_params: &[mojito_ast::ast::TypeParam],
        pattern: &Ty,
        actual: &Ty,
        explicit: &[super::super::type_resolution::ExplicitStructOrigin],
        bindings: &mut ConstructorOriginBindings,
    ) -> Result<(), TypeError> {
        let slot_of = |id: &OriginParamId| source_params.get(id.0 as usize);
        match (pattern.kind(), actual.kind()) {
            (pattern_kind, actual_kind)
                if let (
                    Some((
                        _,
                        PointerOrigin::Param {
                            id,
                            interior,
                            subtree,
                            ..
                        },
                    )),
                    Some((_, actual_origin)),
                ) = (pattern_kind.as_pointer(), actual_kind.as_pointer()) =>
            {
                let Some(slot) = slot_of(&id) else {
                    return Ok(());
                };
                let requires_mut = matches!(
                    slot.origin_mutability.as_ref().map(|e| &e.kind),
                    Some(ExprKind::Bool(true))
                );
                if requires_mut && actual_origin.statically_mutable() == Some(false) {
                    return Err(TypeError::TypeMismatch {
                        expected: format!(
                            "a mutable-origin pointer for parameter '{}' of '{struct_name}'",
                            slot.name
                        ),
                        found: "an immutable-origin pointer".to_string(),
                        context: context(),
                    });
                }
                if let Some(explicit) = explicit.iter().find(|origin| origin.id == id) {
                    let within = match actual_origin.as_origin() {
                        Some(actual) => origin_is_within(&actual, &explicit.origin),
                        None => {
                            matches!(explicit.origin, Origin::Untracked { .. })
                                || (actual_origin == PointerOrigin::Static
                                    && explicit.origin == Origin::Static)
                        }
                    };
                    if !within {
                        return Err(TypeError::TypeMismatch {
                            expected: format!(
                                "a pointer whose origin lies within the supplied '{}' argument",
                                slot.name
                            ),
                            found: format!("a pointer of origin {actual_origin:?}"),
                            context: context(),
                        });
                    }
                }
                // A projected parameter origin (`Self.origin._get_owned_interior[..]`)
                // names a domain below the binder; it is checked, not bound.
                if !interior.is_empty() || subtree {
                    return Ok(());
                }
                bindings.bind_pointer(id, &actual_origin, || {
                    conflicting_binding(struct_name, &slot.name)
                })
            }
            (TyKind::Ref(declared), TyKind::Ref(reference)) => {
                let Origin::Param(id) = &declared.origin else {
                    return Ok(());
                };
                let Some(slot) = slot_of(id) else {
                    return Ok(());
                };
                if let Some(explicit) = explicit.iter().find(|origin| origin.id == *id)
                    && !origin_is_within(&reference.origin, &explicit.origin)
                {
                    return Err(TypeError::TypeMismatch {
                        expected: format!(
                            "a reference whose origin lies within the supplied '{}' argument",
                            slot.name
                        ),
                        found: format!("a reference of origin {:?}", reference.origin),
                        context: context(),
                    });
                }
                bindings.bind_origin(*id, &reference.origin, || {
                    conflicting_binding(struct_name, &slot.name)
                })
            }
            (
                TyKind::Struct(pattern_name, pattern_args),
                TyKind::Struct(actual_name, actual_args),
            ) if !pattern.is_simd()
                && pattern_name == actual_name
                && pattern_args.len() == actual_args.len() =>
            {
                for (pattern, actual) in pattern_args.iter().zip(actual_args) {
                    match (pattern, actual) {
                        (TyArg::Ty(pattern), TyArg::Ty(actual)) => Self::bind_origin_pattern(
                            struct_name,
                            context,
                            source_params,
                            pattern,
                            actual,
                            explicit,
                            bindings,
                        )?,
                        (TyArg::Origin(Origin::Param(id)), TyArg::Origin(actual)) => {
                            // A binder past the struct's is the
                            // constructor's own (`other: V[Self.T, src]`),
                            // which the source's tail solves.
                            let Some(slot) = slot_of(id) else {
                                bindings.bind_origin(*id, actual, || TypeError::BadCall {
                                    func: struct_name.to_string(),
                                    reason: format!("{} binds conflicting origins", context()),
                                })?;
                                continue;
                            };
                            if let Some(explicit) = explicit.iter().find(|origin| origin.id == *id)
                                && !matches!(actual, Origin::Unbound)
                                && !origin_is_within(actual, &explicit.origin)
                            {
                                return Err(TypeError::TypeMismatch {
                                    expected: format!(
                                        "a value whose origin lies within the supplied '{}' argument",
                                        slot.name
                                    ),
                                    found: format!("a value of origin {actual:?}"),
                                    context: context(),
                                });
                            }
                            bindings.bind_origin(*id, actual, || {
                                conflicting_binding(struct_name, &slot.name)
                            })?;
                        }
                        _ => {}
                    }
                }
                Ok(())
            }
            (TyKind::Tuple(patterns), TyKind::Tuple(actuals))
                if patterns.len() == actuals.len() =>
            {
                for (pattern, actual) in patterns.iter().zip(actuals) {
                    Self::bind_origin_pattern(
                        struct_name,
                        context,
                        source_params,
                        pattern,
                        actual,
                        explicit,
                        bindings,
                    )?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// Whether a parameter type names a callee origin binder: a `Pointer[T, o]`,
/// a `ref[o]` referent, or a struct carrying `o` in its origin tail.
pub(in crate::checker) fn names_origin_binder(parameter: &Ty) -> bool {
    mojito_types::types::mentions(parameter, &|candidate| match candidate.kind() {
        handle if let Some((_, origin)) = handle.pointer_or_lit_ref() => {
            matches!(origin, PointerOrigin::Param { .. })
        }
        TyKind::Ref(reference) => matches!(reference.origin, Origin::Param(_)),
        struct_ty if let Some((_, arguments)) = struct_ty.as_non_simd_struct() => arguments
            .iter()
            .any(|argument| matches!(argument, TyArg::Origin(Origin::Param(_)))),
        _ => false,
    })
}

/// Bind the callee origin binders a parameter type's struct tails (and
/// reference origins) name from the actual's, recursing through type
/// arguments and tuples. Pointer binders are the pointer solver's.
fn bind_callable_tail_pattern(
    pattern: &Ty,
    actual: &Ty,
    bindings: &mut ConstructorOriginBindings,
    conflict: &dyn Fn() -> TypeError,
) -> Result<(), TypeError> {
    match (pattern.kind(), actual.kind()) {
        // A pointer nested in a type argument (`Layout[Pointer[Int, o]]`)
        // binds its binder from the actual's provenance; a projected binder
        // (`o._get_owned_interior["element"]`) binds the provenance below
        // the projection, and one the actual does not end in is checked by
        // coercion, not bound.
        (pattern_kind, actual_kind)
            if let Some(((pattern_element, declared), (element, provenance))) =
                pattern_kind.matching_pointer_parts(actual_kind) =>
        {
            if let PointerOrigin::Param {
                id,
                interior,
                subtree,
                ..
            } = declared
                && let Some(base) = provenance.without_projection(&interior, subtree)
            {
                bindings.bind_pointer(id, &base, conflict)?;
            }
            bind_callable_tail_pattern(&pattern_element, &element, bindings, conflict)
        }
        (TyKind::Struct(pattern_name, pattern_args), TyKind::Struct(actual_name, actual_args))
            if !pattern.is_simd()
                && pattern_name == actual_name
                && pattern_args.len() == actual_args.len() =>
        {
            for (pattern, actual) in pattern_args.iter().zip(actual_args) {
                match (pattern, actual) {
                    (TyArg::Ty(pattern), TyArg::Ty(actual)) => {
                        bind_callable_tail_pattern(pattern, actual, bindings, conflict)?;
                    }
                    (TyArg::Origin(Origin::Param(id)), TyArg::Origin(actual)) => {
                        bindings.bind_origin(*id, actual, conflict)?;
                    }
                    _ => {}
                }
            }
            Ok(())
        }
        (TyKind::Ref(declared), TyKind::Ref(reference)) => {
            if let Origin::Param(id) = &declared.origin {
                bindings.bind_origin(*id, &reference.origin, conflict)?;
            }
            bind_callable_tail_pattern(&declared.referent, &reference.referent, bindings, conflict)
        }
        (TyKind::Tuple(patterns), TyKind::Tuple(actuals)) if patterns.len() == actuals.len() => {
            for (pattern, actual) in patterns.iter().zip(actuals) {
                bind_callable_tail_pattern(pattern, actual, bindings, conflict)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// The origin a struct tail records for a bound slot: the storage identity,
/// without the subtree projection a pointer's loan domain may carry.
fn tail_origin(origin: Origin) -> Origin {
    match origin {
        Origin::Place(place) => Origin::Place(place.without_subtree()),
        Origin::Union(members) => Origin::union(members.into_iter().map(tail_origin)),
        Origin::Immutable(inner) => Origin::immutable(tail_origin(*inner)),
        other => other,
    }
}

/// The mutability of a reference over `origin`: its owner's for a place,
/// mutable for a union only when every member is.
fn origin_mutability(
    origin: &Origin,
    owner_mutability: &dyn Fn(mojito_types::origin::OwnerId) -> Mutability,
) -> Option<Mutability> {
    match origin {
        Origin::Place(place) => Some(owner_mutability(place.root)),
        Origin::Untracked { mutable } => Some(if *mutable {
            Mutability::Mutable
        } else {
            Mutability::Immutable
        }),
        Origin::Static | Origin::Immutable(_) => Some(Mutability::Immutable),
        Origin::Union(members) if !members.is_empty() => {
            let mut all = members
                .iter()
                .map(|member| origin_mutability(member, owner_mutability));
            Some(
                if all.all(|mutability| mutability == Some(Mutability::Mutable)) {
                    Mutability::Mutable
                } else {
                    Mutability::Immutable
                },
            )
        }
        _ => None,
    }
}

/// The origin an untracked pointer provenance binds a struct slot to.
const fn untracked_pointer_origin(provenance: &PointerOrigin) -> Option<Origin> {
    match provenance {
        PointerOrigin::Static => Some(Origin::Static),
        PointerOrigin::Untracked { mutable } | PointerOrigin::UnsafeAny { mutable } => {
            Some(Origin::Untracked { mutable: *mutable })
        }
        PointerOrigin::Place { .. }
        | PointerOrigin::Param { .. }
        | PointerOrigin::SelfPlace { .. } => None,
    }
}

fn conflicting_binding(struct_name: &str, slot: &str) -> TypeError {
    TypeError::BadCall {
        func: struct_name.to_string(),
        reason: format!("arguments bind conflicting origins for parameter '{slot}'"),
    }
}

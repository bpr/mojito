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

/// The struct origin binders one construction binds: pointer provenance for
/// substitution into pointer-typed parameters, and the origin every bound
/// binder names for the constructed value's tail.
#[derive(Default)]
pub(in crate::checker) struct ConstructorOriginBindings {
    pub(in crate::checker) pointers: HashMap<OriginParamId, PointerOrigin>,
    pub(in crate::checker) origins: HashMap<OriginParamId, Origin>,
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

    /// [`Self::substitute`] over a parameter list.
    pub(in crate::checker) fn substitute_all(&self, tys: &[Ty]) -> Vec<Ty> {
        tys.iter().map(|ty| self.substitute(ty)).collect()
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

    fn bind_pointer(
        &mut self,
        id: OriginParamId,
        provenance: &PointerOrigin,
        conflict: impl Fn() -> TypeError,
    ) -> Result<(), TypeError> {
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
        if matches!(origin, Origin::Unbound) {
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
        self.struct_instance_type(name, arguments)
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
        Ok(bindings)
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
            let actual = match (pattern, actual) {
                (Ty::Ref(_), Ty::Ref(_)) => actual,
                (Ty::Ref(declared), _) => match self.reference_argument_origin(expression) {
                    Some(origin) => {
                        reference_actual = Ty::Ref(mojito_types::origin::RefTy {
                            referent: declared.referent.clone(),
                            origin,
                            mutability: declared.mutability,
                        });
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
    ) -> Result<ConstructorOriginBindings, TypeError> {
        let mut bindings = ConstructorOriginBindings {
            pointers: Self::bind_callable_pointer_origins(callee, names, params, bound)?,
            origins: HashMap::new(),
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
    /// not bound. `bound` pairs a parameter index with the actual type.
    pub(in crate::checker) fn bind_callable_pointer_origins(
        callee: &str,
        names: &[String],
        params: &[Ty],
        bound: &[(usize, &Ty)],
    ) -> Result<HashMap<OriginParamId, PointerOrigin>, TypeError> {
        let mut bindings: HashMap<OriginParamId, PointerOrigin> = HashMap::new();
        for (index, actual) in bound {
            let (
                Some(Ty::Pointer {
                    origin:
                        PointerOrigin::Param {
                            id,
                            mutability,
                            interior,
                            subtree,
                        },
                    ..
                }),
                Ty::Pointer {
                    origin: actual_origin,
                    ..
                },
            ) = (params.get(*index), actual)
            else {
                continue;
            };
            let parameter = names.get(*index).map_or("?", String::as_str);
            if *mutability == Mutability::Mutable
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
            if !interior.is_empty() || *subtree {
                continue;
            }
            match bindings.get(id) {
                None => {
                    bindings.insert(*id, actual_origin.clone());
                }
                Some(existing) if existing == actual_origin => {}
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
        match (pattern, actual) {
            (
                Ty::Pointer {
                    origin:
                        PointerOrigin::Param {
                            id,
                            interior,
                            subtree,
                            ..
                        },
                    ..
                },
                Ty::Pointer {
                    origin: actual_origin,
                    ..
                },
            ) => {
                let Some(slot) = slot_of(id) else {
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
                if let Some(explicit) = explicit.iter().find(|origin| origin.id == *id) {
                    let within = match actual_origin.as_origin() {
                        Some(actual) => origin_is_within(&actual, &explicit.origin),
                        None => matches!(explicit.origin, Origin::Untracked { .. }),
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
                if !interior.is_empty() || *subtree {
                    return Ok(());
                }
                bindings.bind_pointer(*id, actual_origin, || {
                    conflicting_binding(struct_name, &slot.name)
                })
            }
            (Ty::Ref(declared), Ty::Ref(reference)) => {
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
            (Ty::Struct(pattern_name, pattern_args), Ty::Struct(actual_name, actual_args))
                if pattern_name == actual_name && pattern_args.len() == actual_args.len() =>
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
                            let Some(slot) = slot_of(id) else {
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
            (Ty::Tuple(patterns), Ty::Tuple(actuals)) if patterns.len() == actuals.len() => {
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

/// Bind the callee origin binders a parameter type's struct tails (and
/// reference origins) name from the actual's, recursing through type
/// arguments and tuples. Pointer binders are the pointer solver's.
fn bind_callable_tail_pattern(
    pattern: &Ty,
    actual: &Ty,
    bindings: &mut ConstructorOriginBindings,
    conflict: &dyn Fn() -> TypeError,
) -> Result<(), TypeError> {
    match (pattern, actual) {
        (Ty::Struct(pattern_name, pattern_args), Ty::Struct(actual_name, actual_args))
            if pattern_name == actual_name && pattern_args.len() == actual_args.len() =>
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
        (Ty::Ref(declared), Ty::Ref(reference)) => {
            if let Origin::Param(id) = &declared.origin {
                bindings.bind_origin(*id, &reference.origin, conflict)?;
            }
            bind_callable_tail_pattern(&declared.referent, &reference.referent, bindings, conflict)
        }
        (Ty::Tuple(patterns), Ty::Tuple(actuals)) if patterns.len() == actuals.len() => {
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
        other => other,
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

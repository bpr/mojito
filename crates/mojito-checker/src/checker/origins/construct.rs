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
use mojito_types::origin::{Mutability, Origin, OriginParamId};
use mojito_types::types::TyKind;

/// The struct origin binders one construction binds: each binder's one
/// origin, and the `mut` a pointer argument bound it with.
#[derive(Default)]
pub(in crate::checker) struct ConstructorOriginBindings {
    pub(in crate::checker) bound: OriginBindings,
    /// The capability a `ref` argument lent a binder, where it is not its
    /// origin's storage's: a pointer dereference's is the pointer's.
    references: HashMap<OriginParamId, Mutability>,
    /// The binders this binding binds.
    binders: CalleeBinders,
}

/// The value bindings a call substitutes into its callee's types
/// ([`Checker::call_values`]): `values`, and the callee `mut=` binders the
/// call leaves `open`, whose reads [`Checker::defer_mutability_reads`]
/// defers.
#[derive(Default)]
pub(in crate::checker) struct CallValues {
    pub(in crate::checker) values: DependentValues,
    pub(in crate::checker) open: Vec<mojito_types::param_expr::ParamId>,
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

/// What a construction supplies beside the arguments its selected
/// initializer binds: the application's explicit origins and `mut=`
/// keywords (`Span[Byte, origin_of(s)](…)`, `Origin[mut=False](…)`), and
/// the initializer's own origin binders its instantiation already bound
/// (`v: Origin` binding `v`'s `_mlir_origin`).
#[derive(Clone, Copy)]
pub(in crate::checker) struct StructApplication<'a> {
    pub(in crate::checker) origins: &'a [super::super::BoundOrigin],
    pub(in crate::checker) mutabilities: &'a [(mojito_types::param_expr::ParamId, CtValue)],
    pub(in crate::checker) own: &'a [(OriginParamId, Origin)],
}

/// The origins a construction's application supplies, checked against its
/// arguments' origins in the enclosing method's frame: the receiver's
/// symbolic origin (`origin_of(self)`) and a place rooted at the receiver
/// name the same storage, as upstream's one `self_is_origin` does.
#[derive(Clone, Copy)]
pub(in crate::checker) struct SuppliedOrigins<'a> {
    origins: &'a [super::super::BoundOrigin],
    receiver: Option<&'a Origin>,
}

impl SuppliedOrigins<'_> {
    /// The origin the application supplies for the binder `id`.
    fn find(&self, id: OriginParamId) -> Option<&super::super::BoundOrigin> {
        self.origins.iter().find(|origin| origin.id == id)
    }

    /// Whether `actual` converts to the supplied `allowed`, each read with
    /// `origin_of(self)` at the receiver's place: every member of `actual`
    /// must be a member of `allowed` itself, as upstream converts an argument
    /// to its parameter's type, so a field's origin is not its owner's,
    /// while an owner's interior storage is ([`origin_converts_to`]).
    fn admits(&self, actual: &Origin, allowed: &Origin) -> bool {
        let here = |origin: &Origin| {
            self.receiver.map_or_else(
                || origin.clone(),
                |receiver| super::rebase_self_origin(origin, receiver),
            )
        };
        origin_converts_to(&here(actual), &here(allowed))
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

/// The origin binders among `decls` whose `mut=` binder a bracket fixed
/// to `True` before the arguments bind them, as upstream binds explicit
/// parameters first: by a `mut=` argument among `mutabilities` (`f[m=True]`,
/// `V[m=True]`), or by a mutable origin `origins` binds to a sibling binder
/// that `mut=` binder governs too (`f[t=MutUntrackedOrigin]` over `o, t:
/// Origin[mut=m]`).
pub(in crate::checker) fn bracket_mutable_binders<'a>(
    decls: &[ParamDecl],
    mutabilities: impl IntoIterator<Item = (&'a mojito_types::param_expr::ParamId, &'a CtValue)>,
    origins: &[super::super::BoundOrigin],
) -> Vec<OriginParamId> {
    let governor = |decl: &ParamDecl| match decl {
        ParamDecl::Origin {
            mutability: mojito_types::types::OriginMutability::Binder(binder),
            ..
        } => Some(binder.id.clone()),
        _ => None,
    };
    let fixed: Vec<_> = mutabilities
        .into_iter()
        .filter(|(_, value)| **value == CtValue::Bool(true))
        .map(|(id, _)| id.clone())
        .chain(
            origins
                .iter()
                .filter(|bound| bound.mutability == Some(Mutability::Mutable))
                .filter_map(|bound| decls.get(bound.slot).and_then(governor)),
        )
        .collect();
    decls
        .iter()
        .filter_map(|decl| match decl {
            ParamDecl::Origin { origin, .. }
                if governor(decl).is_some_and(|binder| fixed.contains(&binder))
                    && !origins.iter().any(|bound| bound.id == *origin) =>
            {
                Some(*origin)
            }
            _ => None,
        })
        .collect()
}

/// The parameter of `decls`, a callee's or a struct's whole declared list,
/// that upstream reports first as failing to infer among its origin binders
/// and their `mut=` binders, spelled as upstream spells it: a `mut=` binder
/// `m` no bracket (`value_bound`), default, or bound sibling decides by its
/// name; a bare `o: Origin`'s implicit one as `mut`; a raw binder `r` (`r:
/// _lit_origin_type_of_mut[m]`) as `origin_of(r)`; and any other origin
/// binder as its type's implicit `origin_of(_mlir_origin)`. An origin
/// binder is bound when `origin_bound` holds of it, and a raw one also when
/// the sibling naming it as its `_mlir_origin` is. An `OriginSet` binder,
/// which a capturing callable's environment binds, is never reported.
pub(in crate::checker) fn uninferred_origin_parameter(
    decls: &[ParamDecl],
    origin_bound: &dyn Fn(OriginParamId) -> bool,
    value_bound: &dyn Fn(&mojito_types::param_expr::ParamId) -> bool,
) -> Option<String> {
    use mojito_types::types::OriginMutability;
    let bound = |id: OriginParamId| {
        origin_bound(id) || ParamDecl::raw_origin_source(decls, id).is_some_and(origin_bound)
    };
    let governs_bound = |binder: &mojito_types::param_expr::ParamId| {
        decls.iter().any(|decl| {
            matches!(decl, ParamDecl::Origin {
                origin,
                mutability: OriginMutability::Binder(governor),
                ..
            } if governor.id == *binder && bound(*origin))
        })
    };
    // An alias-typed binder's implicit `$mut_o` is spelled as upstream's
    // `Origin` declares it.
    let spelled = |name: &str| {
        mojito_ast::ast::auto_binder_owner(name, "mut")
            .map_or_else(|| name.to_string(), |_| "mut".to_string())
    };
    decls.iter().find_map(|decl| match decl {
        ParamDecl::Value {
            id, name, default, ..
        } if decl.is_origin_mutability_binder(decls) => {
            (default.is_none() && !value_bound(id) && !governs_bound(id)).then(|| spelled(name))
        }
        ParamDecl::Origin {
            mutability: OriginMutability::Set,
            ..
        } => None,
        ParamDecl::Origin { origin, .. } if bound(*origin) => None,
        ParamDecl::Origin {
            name, lit: true, ..
        } => Some(format!(
            "origin_of({})",
            mojito_ast::ast::auto_binder_owner(name, "_mlir_origin")
                .map_or(name.as_str(), |_| "_mlir_origin")
        )),
        ParamDecl::Origin {
            mutability: OriginMutability::Binder(binder),
            ..
        } if binder.is_unbound() && !value_bound(&binder.id) => Some("mut".to_string()),
        ParamDecl::Origin { .. } => Some("origin_of(_mlir_origin)".to_string()),
        _ => None,
    })
}

impl ConstructorOriginBindings {
    /// Substitute the bound binders into a parameter or field type
    /// ([`substitute_struct_origins`]).
    pub(in crate::checker) fn substitute(&self, ty: &Ty) -> Ty {
        substitute_struct_origins(ty, &self.bound)
    }

    /// [`Self::substitute`] for a declared result: a binder this call bound
    /// only through a struct argument's origin tail (`v: View[o]` over
    /// `-> Pointer[T, o]`) takes its `mut` from `binder_mutability` too
    /// ([`OriginBindings::infer_mutabilities`]), as upstream infers `m`
    /// with `o`, so the result's pointers over it bind.
    pub(in crate::checker) fn substitute_result(
        &self,
        ty: &Ty,
        binder_mutability: &dyn Fn(OriginParamId, &Origin) -> Option<Mutability>,
    ) -> Ty {
        let mut bound = self.bound.clone();
        bound.infer_mutabilities(binder_mutability);
        substitute_struct_origins(ty, &bound)
    }

    /// [`Self::substitute`] over a parameter list.
    pub(in crate::checker) fn substitute_all(&self, tys: &[Ty]) -> Vec<Ty> {
        tys.iter().map(|ty| self.substitute(ty)).collect()
    }

    /// Whether the call bound no binder.
    pub(in crate::checker) fn is_empty(&self) -> bool {
        self.bound.is_empty()
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
            address_space: signature.address_space.clone(),
        }
    }

    /// The constructed value's origin tail: each slot's explicit origin where
    /// the application supplied one, else the origin its binder bound, else
    /// unbound.
    pub(in crate::checker) fn bind_origins(
        &self,
        slots: impl Iterator<Item = OriginParamId>,
        tail: &[Origin],
    ) -> Vec<Origin> {
        slots
            .zip(tail)
            .map(|(id, explicit)| match explicit {
                Origin::Unbound => self.tail(id).unwrap_or(Origin::Unbound),
                bound => bound.clone(),
            })
            .collect()
    }

    /// The origin binder `id` names in a constructed value's tail: its
    /// bound origin, a pointer argument's `_subtree` dropped.
    pub(in crate::checker) fn tail(&self, id: OriginParamId) -> Option<Origin> {
        self.bound.origins.get(&id).cloned().map(tail_origin)
    }

    /// The mutability binder `id` was bound with: a pointer argument's,
    /// else the storage its origin names.
    pub(in crate::checker) fn bound_mutability(
        &self,
        id: OriginParamId,
        owner_mutability: &dyn Fn(mojito_types::origin::OwnerId) -> Mutability,
    ) -> Option<Mutability> {
        if let Some((origin, mutability)) = self.pointer_binding(id) {
            return mutability.known().map(Mutability::of).or_else(|| {
                matches!(
                    origin.projection_base(),
                    Origin::Param(_) | Origin::SelfParam
                )
                .then_some(mutability)
            });
        }
        if let Some(mutability) = self.references.get(&id) {
            return Some(*mutability);
        }
        origin_mutability(self.bound.origins.get(&id)?, owner_mutability)
    }

    /// The canonical pointer origin and mutability a pointer argument
    /// bound binder `id` with ([`Origin::canonical_pointer_parts`]).
    fn pointer_binding(&self, id: OriginParamId) -> Option<(Origin, Mutability)> {
        self.bound
            .origins
            .get(&id)?
            .canonical_pointer_parts(self.bound.mutability(id)?)
    }

    fn substitute_sig_origin(
        &self,
        origin: &mojito_types::origin::SigOrigin,
    ) -> mojito_types::origin::SigOrigin {
        use mojito_types::origin::SigOrigin;
        match origin {
            SigOrigin::Bound(bound) => {
                SigOrigin::Bound(super::substitute_origin_params(bound.clone(), &|id| {
                    self.bound.origins.get(&id).cloned()
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

    /// Bind binder `id` to a pointer argument's canonical `origin` and
    /// `mutability`; a second binding must name the same ones.
    fn bind_pointer(
        &mut self,
        id: OriginParamId,
        (origin, mutability): (Origin, Mutability),
        conflict: impl Fn() -> TypeError,
    ) -> Result<(), TypeError> {
        if !self.binders.binds(id) {
            return Ok(());
        }
        match self.pointer_binding(id) {
            None => {
                self.bound.origins.insert(id, origin);
                self.bound
                    .mutabilities
                    .insert(id, mutability.mut_argument());
                Ok(())
            }
            Some(existing) if existing == (origin, mutability) => Ok(()),
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
        match self.bound.origins.get(&id) {
            None => {
                self.bound.origins.insert(id, origin);
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
    /// mutability. A mutability the caller's own `mut=` binder decides is
    /// a read of that binder, which the caller's instance closes. One an
    /// implicit binder of the caller decides (its receiver's or a bare
    /// `ref` parameter's reference binder, or a bare `o: Origin`'s own),
    /// where upstream's call reads the caller's implicit `self_is_mut`,
    /// is that binder's `$origin_mut` read ([`Mutability::mut_argument`]),
    /// and only an undecided one is [`CtValue::Deferred`] of the callee's
    /// binder. A raw origin binder no bracket or place binds takes the
    /// binding of the sibling whose `_mlir_origin=` names it
    /// ([`ParamDecl::raw_origin_source`]), and so does the `mut=` binder it
    /// governs, as upstream infers both from that sibling's type.
    /// `arguments` is returned as it stands when it is no solution of the
    /// argument view.
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
        let own = |id: OriginParamId| {
            bracket
                .get(&id)
                .cloned()
                .or_else(|| bindings.and_then(|bindings| bindings.tail(id)))
        };
        // The binder whose binding `id` takes: its own, else the sibling's
        // whose declared type names it as its `_mlir_origin`.
        let source = |id: OriginParamId| match own(id) {
            Some(_) => id,
            None => ParamDecl::raw_origin_source(decls, id).unwrap_or(id),
        };
        let origin = |id: OriginParamId| own(source(id));
        let bound = |id: OriginParamId| {
            bindings?.bound_mutability(source(id), &|owner| self.owner_reference_mutability(owner))
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
                    TyArg::Val(governed.map_or_else(
                        || CtValue::Deferred(decl.binder()),
                        |id| match bracket.get(&source(id)) {
                            Some(origin) => self.call_mutability_slot(decl, origin, None),
                            None => self.call_mutability_slot(
                                decl,
                                &origin(id).unwrap_or(Origin::Unbound),
                                bound(id),
                            ),
                        },
                    ))
                }
                _ => solved
                    .next()
                    .cloned()
                    .unwrap_or(TyArg::Origin(Origin::Unbound)),
            })
            .collect()
    }

    /// The value an origin's `mut=` binder `decl` holds where its governed
    /// origin is `origin`, as upstream's instance and call carry it in
    /// place: the origin's own when the origin alone decides it (a cast's,
    /// a static origin's `False`), else `bound`, the mutability a
    /// construction's arguments bound, when there is one, else the
    /// origin's own (a place's owner's). A mutability nothing decides
    /// yet (an unbound origin's) or a binder out of scope decides (a `ref`
    /// parameter's) is [`CtValue::Deferred`], following its origin as
    /// `Origin::Unbound` and `Origin::Param` do; a `mut=` binder in scope
    /// (`o: Origin[mut=m]`) decides it as its read, which the instance
    /// holds, as upstream's does.
    pub(in crate::checker) fn origin_mutability_slot(
        &self,
        decl: &ParamDecl,
        origin: &Origin,
        bound: Option<Mutability>,
    ) -> CtValue {
        match self.decided_slot_mutability(origin, bound) {
            Some(Mutability::Mutable) => CtValue::Bool(true),
            Some(Mutability::Immutable) => CtValue::Bool(false),
            Some(Mutability::Param(id))
                if let Some(read) = self.origin_mutability_binder_read(id) =>
            {
                CtValue::Expr(read)
            }
            Some(Mutability::Param(_)) | None => origin.symbolic_cast_mutability().map_or_else(
                || CtValue::Deferred(decl.binder()),
                |read| CtValue::Expr(read.clone()),
            ),
        }
    }

    /// How a call of a callee declaring the whole list `binders`, with
    /// `declared` its solved arguments over that list
    /// ([`Self::declared_call_arguments`]), binds the callee's value
    /// binders in the types it substitutes: over the whole list, its `mut=`
    /// binders among them, as upstream's call binds every declared
    /// parameter, else over the argument view `decls` and its solutions
    /// `arguments`. A `mut=` binder the call leaves open (a
    /// [`CtValue::Deferred`] value) is listed for
    /// [`Self::defer_mutability_reads`].
    pub(in crate::checker) fn call_values(
        &self,
        (binders, declared): (&[ParamDecl], &[TyArg]),
        decls: &[ParamDecl],
        arguments: &[TyArg],
    ) -> CallValues {
        let whole = declared.len() == binders.len();
        let values = if whole {
            DependentValues::own(&self.param_context, binders, declared)
        } else {
            DependentValues::own(&self.param_context, decls, arguments)
        };
        let open = binders
            .iter()
            .enumerate()
            .filter(|(index, decl)| {
                decl.is_origin_mutability_binder(binders)
                    && (!whole || matches!(declared[*index], TyArg::Val(CtValue::Deferred(_))))
            })
            .map(|(_, decl)| decl.id().clone())
            .collect();
        CallValues { values, open }
    }

    /// `ty`, a callee's type a call substituted, with every struct
    /// instance's `mut=` slot that still reads a callee `mut=` binder the
    /// call left `open` ([`Self::call_values`]) deferred again, as its
    /// governed origin is: the callee's binder is out of the caller's scope.
    pub(in crate::checker) fn defer_mutability_reads(
        &self,
        ty: &Ty,
        open: &[mojito_types::param_expr::ParamId],
    ) -> Ty {
        mojito_types::types::defer_mutability_reads(ty, open, &|name| {
            self.structs.get(name).map(|info| info.params.as_slice())
        })
    }

    /// `ty` resolved at a call's `values` ([`Self::call_values`]): its
    /// dependent values closed, and every `mut=` read the call left open
    /// deferred.
    pub(in crate::checker) fn resolve_call_dependent_ty(
        &self,
        ty: &Ty,
        values: &CallValues,
    ) -> Result<Ty, TypeError> {
        self.resolve_dependent_ty(ty, &values.values)
            .map(|resolved| self.defer_mutability_reads(&resolved, &values.open))
    }

    /// The value a struct application's `mut=` slot `decl` holds when it
    /// holds [`CtValue::Deferred`], read as its governed origin's
    /// mutability, as `Origin[mut=mut]` ties the two: the `Bool` or
    /// binder read the origin decides. `None`
    /// where nothing decides it yet (an unbound origin's).
    pub(in crate::checker) fn deferred_mutability_value(
        &self,
        decls: &[ParamDecl],
        arguments: &[TyArg],
        decl: &ParamDecl,
    ) -> Option<CtValue> {
        let (_, TyArg::Origin(origin)) =
            ParamDecl::aligned_pairs(decls, arguments).find(|(candidate, _)| {
                matches!(candidate, ParamDecl::Origin {
                    mutability: mojito_types::types::OriginMutability::Binder(binder),
                    ..
                } if binder.id == *decl.id())
            })?
        else {
            return None;
        };
        Some(self.origin_mutability_slot(decl, origin, None))
            .filter(|value| !matches!(value, CtValue::Deferred(_)))
    }

    /// The value each origin `mut=` binder among `decls`, a callee's whole
    /// declared list, takes from the origin `bindings` bound its origin to.
    pub(in crate::checker) fn own_origin_mutabilities(
        &self,
        decls: &[ParamDecl],
        bindings: &ConstructorOriginBindings,
    ) -> Vec<(mojito_types::param_expr::ParamId, CtValue)> {
        ParamDecl::mutability_binders(decls)
            .filter_map(|decl| {
                let governed = decls.iter().find_map(|candidate| match candidate {
                    ParamDecl::Origin {
                        origin,
                        mutability: mojito_types::types::OriginMutability::Binder(binder),
                        ..
                    } if binder.id == *decl.id() => Some(*origin),
                    _ => None,
                })?;
                let origin = bindings.bound.origins.get(&governed)?;
                let bound = bindings
                    .bound_mutability(governed, &|owner| self.owner_reference_mutability(owner));
                Some((
                    decl.id().clone(),
                    self.origin_mutability_slot(decl, origin, bound),
                ))
            })
            .collect()
    }

    /// The whole parameter list of the struct whose members are being
    /// checked, the binders `Self` holds.
    pub(in crate::checker) fn self_struct_params(&self) -> Option<&[ParamDecl]> {
        let (owner, _) = self
            .self_ty
            .as_ref()
            .and_then(mojito_types::types::nominal_parts)?;
        self.structs.get(owner).map(|info| info.params.as_slice())
    }

    /// [`Self::own_origin_mutabilities`] keyed by binder name, for a
    /// by-name value substitution, each binder the origins leave undecided
    /// left out.
    pub(in crate::checker) fn own_mutability_values(
        &self,
        decls: &[ParamDecl],
        bindings: &ConstructorOriginBindings,
    ) -> Vec<(String, CtValue)> {
        self.own_origin_mutabilities(decls, bindings)
            .into_iter()
            .filter(|(_, value)| !matches!(value, CtValue::Deferred(_)))
            .filter_map(|(id, value)| {
                decls
                    .iter()
                    .find(|decl| *decl.id() == id)
                    .map(|decl| (decl.binder().name.to_string(), value))
            })
            .collect()
    }

    /// The instance of the struct `info` whose argument binders take `view`
    /// and whose origin binders take `origins`, each origin's `mut=` binder
    /// holding its [`Self::origin_mutability_slot`] value, or the value an
    /// explicit `mut=` keyword gave it (`explicit`).
    pub(in crate::checker) fn place_struct_origins(
        &self,
        decls: &[ParamDecl],
        view: impl IntoIterator<Item = TyArg>,
        origins: impl IntoIterator<Item = Origin>,
        explicit: &[(mojito_types::param_expr::ParamId, CtValue)],
    ) -> Vec<TyArg> {
        ParamDecl::place_origins_with(decls, view, origins, |decl, origin| {
            explicit.iter().find(|(id, _)| id == decl.id()).map_or_else(
                || self.origin_mutability_slot(decl, origin, None),
                |(_, value)| value.clone(),
            )
        })
    }

    /// The checked type of a construction: the solved argument view with
    /// the origins the application (`bracket`) and the arguments bound in
    /// place. An origin binder neither the application spells nor the
    /// arguments bound fails to infer, as upstream infers a parent struct's
    /// parameters from the initializer's arguments alone, named in
    /// upstream's words ([`uninferred_origin_parameter`]).
    pub(in crate::checker) fn constructed_type(
        &self,
        name: &str,
        view: Vec<TyArg>,
        bracket: &super::super::type_resolution::StructBracket,
        bindings: &ConstructorOriginBindings,
    ) -> Result<Ty, TypeError> {
        let arguments = match self.structs.get(name) {
            Some(info) => {
                let bound =
                    bindings.bind_origins(info.origin_decls().map(|(id, _)| id), &bracket.origins);
                let origin_bound = |id: OriginParamId| {
                    bracket
                        .explicit
                        .origins()
                        .iter()
                        .any(|spelled| spelled.id == id)
                        || bound
                            .iter()
                            .zip(info.origin_decls())
                            .any(|(origin, (slot, _))| {
                                slot == id && !matches!(origin, Origin::Unbound)
                            })
                };
                if let Some((_, decl)) = info
                    .origin_decls()
                    .find(|(id, decl)| !decl.is_origin_set() && !origin_bound(*id))
                {
                    let parameter =
                        uninferred_origin_parameter(&info.params, &origin_bound, &|binder| {
                            bracket.mutabilities.iter().any(|(id, _)| id == binder)
                        })
                        .unwrap_or_else(|| decl.name().to_string());
                    return Err(TypeError::BadCall {
                        func: name.to_string(),
                        reason: format!(
                            "failed to infer parameter '{parameter}' of parent struct '{name}'"
                        ),
                    });
                }
                let origins = bound
                    .into_iter()
                    .zip(info.origin_decls())
                    .map(|(origin, (_, decl))| self.slot_converted_origin(decl, origin))
                    .collect::<Vec<_>>();
                ParamDecl::place_origins_with(
                    &info.params,
                    view.into_iter()
                        .filter(|argument| !matches!(argument, TyArg::Origin(_))),
                    origins,
                    |decl, origin| {
                        if let Some((_, value)) =
                            bracket.mutabilities.iter().find(|(id, _)| id == decl.id())
                        {
                            return value.clone();
                        }
                        let bound = info
                            .origin_decls()
                            .find(|(_, candidate)| match candidate {
                                ParamDecl::Origin {
                                    mutability:
                                        mojito_types::types::OriginMutability::Binder(binder),
                                    ..
                                } => binder.id == *decl.id(),
                                _ => false,
                            })
                            .and_then(|(id, _)| {
                                bindings.bound_mutability(id, &|owner| {
                                    self.owner_reference_mutability(owner)
                                })
                            });
                        self.origin_mutability_slot(decl, origin, bound)
                    },
                )
            }
            None => view,
        };
        Ok(Self::struct_instance_type(name, arguments))
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
        params: &[ParamDecl],
        sig: &MethodSig,
        bound: &[(usize, &Expr, &Ty)],
        arg_tys: &[Ty],
        application: StructApplication<'_>,
    ) -> Result<ConstructorOriginBindings, TypeError> {
        let receiver = self.receiver_origin();
        let explicit = SuppliedOrigins {
            origins: application.origins,
            receiver: receiver.as_ref(),
        };
        let mut bindings = ConstructorOriginBindings::default();
        for ((index, expression, pattern), actual) in bound.iter().zip(arg_tys) {
            let parameter = sig.names.get(*index).map_or("?", String::as_str);
            let context = || format!("argument '{parameter}' to '{struct_name}.{callee}'");
            Self::bind_origin_pattern(
                struct_name,
                &context,
                params,
                pattern,
                actual,
                explicit,
                &mut bindings,
            )?;
            // A `ref [Self.o]` parameter binds its binder to the argument's
            // origin: a reference-valued place (a `ref [o]` parameter of the
            // enclosing signature) carries its declared origin, any other
            // place its own.
            if let Some(Some((id, _))) = sig.origin_binders.get(*index)
                && let Some(slot) = super::super::struct_origin_decl(params, *id)
                && let Some((actual, lent)) = self.reference_argument(expression)
            {
                if let Some(supplied) = explicit.find(*id)
                    && !explicit.admits(&actual, &supplied.origin)
                {
                    return Err(TypeError::TypeMismatch {
                        expected: format!(
                            "a reference whose origin is the supplied '{}' argument",
                            slot.name()
                        ),
                        found: format!(
                            "a reference of origin {}",
                            self.display_origin_with_names(&actual)
                        ),
                        context: context(),
                    });
                }
                bindings.bind_origin(*id, &actual, || {
                    conflicting_binding(struct_name, slot.name())
                })?;
                // A `Pointer`'s place origin folds with a decided `mut`
                // only, so a place reached through a `ref` whose `mut=`
                // binder is out of scope (a bare `ref` parameter's) lends
                // it immutably.
                let lent = match lent {
                    Some(Mutability::Param(binder))
                        if struct_name == mojito_types::types::POINTER_TYPE_NAME
                            && matches!(actual, Origin::Place(_))
                            && self.origin_mutability_binder_read(binder).is_none() =>
                    {
                        Some(Mutability::Immutable)
                    }
                    Some(Mutability::Param(_)) | None => None,
                    concrete => concrete,
                };
                if let Some(lent) = lent
                    && bindings.binders.binds(*id)
                {
                    bindings.references.entry(*id).or_insert(lent);
                }
            }
        }
        // A struct origin binder whose `mut=` binder the bracket fixed to
        // `True` (`V[m=True](q)`) takes no immutable argument origin.
        if let Some(slot) = bracket_mutable_binders(
            params,
            application
                .mutabilities
                .iter()
                .map(|(id, value)| (id, value)),
            application.origins,
        )
        .into_iter()
        .find(|id| {
            bindings.bound_mutability(*id, &|owner| self.owner_reference_mutability(owner))
                == Some(Mutability::Immutable)
        })
        .and_then(|id| super::super::struct_origin_decl(params, id))
        {
            return Err(TypeError::TypeMismatch {
                expected: format!(
                    "a mutable origin for parameter '{}' of '{struct_name}'",
                    slot.name()
                ),
                found: "an immutable origin".to_string(),
                context: format!("argument to '{struct_name}.{callee}'"),
            });
        }
        self.bind_refined_origins(struct_name, sig, application, &mut bindings)?;
        Ok(bindings)
    }

    /// Bind the struct origin slots an initializer's `out self` refines to
    /// the origin the refinement spells (`out self: StaticString` on
    /// `StringSpan[origin]` binds `origin` to the static origin), as the
    /// pin's call result is its declared result with the call's solutions
    /// substituted: an initializer's own binder (`-> ImmOrigin[_mlir_origin=
    /// v._mlir_origin]`) takes what the call bound it to, and a known `mut=`
    /// slot the refinement spells casts the slot's origin to it. An
    /// explicitly applied origin naming different storage, or an explicit
    /// `mut=` naming a different mutability, makes the initializer no
    /// candidate.
    pub(in crate::checker) fn bind_refined_origins(
        &self,
        struct_name: &str,
        sig: &MethodSig,
        application: StructApplication<'_>,
        bindings: &mut ConstructorOriginBindings,
    ) -> Result<(), TypeError> {
        let (Some(refined), Some(info)) = (&sig.refined_self, self.structs.get(struct_name)) else {
            return Ok(());
        };
        let refined = mojito_types::types::nominal_parts(refined)
            .map(|(_, arguments)| arguments.into_owned())
            .unwrap_or_default();
        let own: Vec<OriginParamId> = declared_origin_binders(&sig.declared).collect();
        for (defined, decl) in refined.iter().zip(&info.params) {
            let (
                TyArg::Origin(origin),
                ParamDecl::Origin {
                    origin: id,
                    mutability,
                    ..
                },
            ) = (defined, decl)
            else {
                continue;
            };
            // A struct binder or `Self` leaves the slot unrefined.
            if matches!(origin, Origin::SelfParam)
                || matches!(origin, Origin::Param(param) if !own.contains(param))
            {
                continue;
            }
            // The constructor's own binders the arguments or its bracket
            // solved (`out self: V[Self.T, ImmOrigin(src)]`) are the call's.
            let mut origin = super::substitute_origin_params(origin.clone(), &|binder| {
                bindings.tail(binder).or_else(|| {
                    application
                        .own
                        .iter()
                        .find(|(candidate, _)| *candidate == binder)
                        .map(|(_, origin)| origin.clone())
                })
            });
            if matches!(origin, Origin::Param(param) if own.contains(&param)) {
                continue;
            }
            if let mojito_types::types::OriginMutability::Binder(binder) = mutability
                && let Some(TyArg::Val(CtValue::Bool(mutable))) = info
                    .params
                    .iter()
                    .position(|candidate| *candidate.id() == binder.id)
                    .and_then(|slot| refined.get(slot))
            {
                if let Some((_, spelled)) = application
                    .mutabilities
                    .iter()
                    .find(|(candidate, _)| *candidate == binder.id)
                    .filter(|(_, spelled)| *spelled != CtValue::Bool(*mutable))
                {
                    return Err(TypeError::TypeMismatch {
                        expected: spelled.to_string(),
                        found: mutable.to_string(),
                        context: format!(
                            "parameter '{}' of the return type '{}'",
                            binder.name,
                            sig.refined_self
                                .as_ref()
                                .map_or_else(String::new, Ty::to_string)
                        ),
                    });
                }
                origin = self.fold_origin_mut_cast(&Origin::mut_cast(
                    origin,
                    mojito_types::types::LitOriginMut::Known(*mutable),
                ));
            }
            let conflict = || conflicting_binding(struct_name, decl.name());
            if application
                .origins
                .iter()
                .any(|applied| applied.id == *id && !origin.coerces_to(&applied.origin))
            {
                return Err(conflict());
            }
            bindings.bind_origin(*id, &origin, conflict)?;
        }
        Ok(())
    }

    /// Bind the struct origin binders a `@fieldwise_init` construction's
    /// field types name from the arguments filling those fields, checking
    /// them against the explicitly applied origins like a hand-written
    /// constructor's parameters. A field declared `ref` binds from the
    /// argument's place, as its `ref` parameter retains it.
    pub(in crate::checker) fn bind_fieldwise_origins(
        &self,
        struct_name: &str,
        info: &crate::checker::StructInfo,
        args: &[Expr],
        arg_tys: &[Ty],
        explicit: &[super::super::BoundOrigin],
    ) -> Result<ConstructorOriginBindings, TypeError> {
        let receiver = self.receiver_origin();
        let explicit = SuppliedOrigins {
            origins: explicit,
            receiver: receiver.as_ref(),
        };
        let mut bindings = ConstructorOriginBindings::default();
        let params = &info.params;
        for (((field, pattern), expression), actual) in info.fields.iter().zip(args).zip(arg_tys) {
            let context = || format!("field '{field}' of '{struct_name}'");
            // A reference field binds from the argument's origin when the
            // argument's own type carries no reference.
            let reference_actual;
            let actual = match (pattern.kind(), actual.kind()) {
                (TyKind::Ref(_), TyKind::Ref(_)) => actual,
                (TyKind::Ref(declared), _) if info.declares_ref_field(field) => {
                    match self.reference_argument_origin(expression) {
                        Some(origin) => {
                            reference_actual = Ty::new(TyKind::Ref(mojito_types::origin::RefTy {
                                referent: declared.referent.clone(),
                                origin,
                                mutability: declared.mutability,
                                address_space: declared.address_space.clone(),
                            }));
                            &reference_actual
                        }
                        None => actual,
                    }
                }
                _ => actual,
            };
            Self::bind_origin_pattern(
                struct_name,
                &context,
                params,
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
        let pointers = Self::bind_callable_pointer_origins(callee, names, params, bound, &binders)?;
        let mut bindings = ConstructorOriginBindings {
            bound: OriginBindings::default(),
            references: HashMap::new(),
            binders,
        };
        for (id, (origin, mutability)) in pointers {
            bindings.bound.origins.insert(id, origin);
            bindings
                .bound
                .mutabilities
                .insert(id, mutability.mut_argument());
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
    ) -> Result<HashMap<OriginParamId, (Origin, Mutability)>, TypeError> {
        let mut bindings: HashMap<OriginParamId, (Origin, Mutability)> = HashMap::new();
        let canonical = |ty: &Ty| {
            ty.kind().pointer_or_lit_ref().and_then(|reference| {
                reference
                    .origin
                    .canonical_pointer_parts(reference.mutability)
            })
        };
        for (index, actual) in bound {
            let (Some((pattern, mutability)), Some(actual_origin)) =
                (params.get(*index).and_then(canonical), canonical(actual))
            else {
                continue;
            };
            let Some((id, projection)) = pattern.binder_projection() else {
                continue;
            };
            if !binders.binds(id) {
                continue;
            }
            let parameter = names.get(*index).map_or("?", String::as_str);
            if mutability == Mutability::Mutable
                && let Some(found) = not_statically_mutable(&actual_origin)
            {
                return Err(TypeError::TypeMismatch {
                    expected: format!(
                        "a mutable-origin pointer for parameter '{parameter}' of '{callee}'"
                    ),
                    found,
                    context: format!("argument '{parameter}' to '{callee}'"),
                });
            }
            if !projection.is_empty() {
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

    /// The mutability `origin` decides by itself: a place's owner's, a
    /// static or cast origin's, a projection's base's, or an enclosing
    /// binder's ([`Self::bound_origin_mutability`]).
    pub(in crate::checker) fn decided_origin_mutability(
        &self,
        origin: &Origin,
    ) -> Option<Mutability> {
        match origin {
            Origin::Projected(base, _) => self.decided_origin_mutability(base),
            Origin::Param(_) => self.bound_origin_mutability(origin),
            // The receiver's origin has the receiver's capability.
            Origin::SelfParam => self
                .lookup_owner("self")
                .map(|owner| self.owner_reference_mutability(owner)),
            _ => origin_mutability(origin, &|owner| self.owner_reference_mutability(owner)),
        }
    }

    /// The origin a struct's origin slot `decl` holds for the origin its
    /// construction bound: an `ImmOrigin` (`Origin[mut=False]`) slot holds
    /// the `ImmOrigin(o)` cast of an `o` not already immutable, as
    /// upstream's implicit conversion of the inferred origin to the slot's
    /// type makes it, and as the slot's explicit annotation resolves.
    fn slot_converted_origin(&self, decl: &ParamDecl, origin: Origin) -> Origin {
        match decl {
            ParamDecl::Origin {
                mutability: mojito_types::types::OriginMutability::Fixed(false),
                ..
            } => self.fold_origin_mut_cast(&Origin::immutable(origin)),
            _ => origin,
        }
    }

    /// The read of the `mut=` binder the enclosing origin binder `id`
    /// declares as its mutability (`o: Origin[mut=mut]`), when that binder
    /// is a value parameter in scope.
    pub(in crate::checker) fn origin_mutability_binder_read(
        &self,
        id: OriginParamId,
    ) -> Option<ParamExpr> {
        self.enclosing_param_decls
            .iter()
            .find_map(|decl| match decl {
                ParamDecl::Origin {
                    origin,
                    mutability: mojito_types::types::OriginMutability::Binder(binder),
                    ..
                } if *origin == id && &*binder.name != "_" => self
                    .own_struct_binder_read(binder)
                    .or_else(|| self.value_parameter_in_scope(&binder.name)),
                _ => None,
            })
    }

    /// The read of `binder` as the enclosing struct's own parameter, the
    /// argument `Self` holds for it ([`ParamDecl::own_argument`]), when the
    /// struct whose members are being checked declares it: a member reads
    /// its struct's binders through `Self`, never as a bare name.
    fn own_struct_binder_read(&self, binder: &ParamRef) -> Option<ParamExpr> {
        self.self_struct_params()?
            .iter()
            .find(|decl| decl.binder().id == binder.id)
            .and_then(|decl| match decl.own_argument() {
                TyArg::Val(CtValue::Expr(read)) => Some(read),
                _ => None,
            })
    }

    /// The origin an argument lends to a `ref [o]` parameter or reference
    /// field: a reference-valued place (a `ref [o]` parameter of the enclosing
    /// signature, a `ref` binding) carries its declared origin, any other
    /// place its own storage, and a temporary the hidden slot it materializes
    /// into.
    fn reference_argument_origin(&self, expression: &Expr) -> Option<Origin> {
        self.reference_argument(expression)
            .map(|(origin, _)| origin)
    }

    /// [`Self::reference_argument_origin`], with the capability the
    /// argument lends where its reference records one: a pointer
    /// dereference lends the pointer's, not its holder's.
    pub(in crate::checker) fn reference_argument(
        &self,
        expression: &Expr,
    ) -> Option<(Origin, Option<Mutability>)> {
        // A `ref [o]` parameter of the enclosing signature lends its binder.
        if let ExprKind::Identifier(name) = &expression.kind
            && let Some((id, _)) = self.lookup_reference_parameter_binder(name)
        {
            return Some((Origin::Param(id), None));
        }
        self.returned_reference_parameter_origin(expression)
            .map(|origin| (origin, None))
            .or_else(|| {
                self.materialized_reference_actual(expression)
                    .ok()
                    .map(|reference| (reference.origin, Some(reference.mutability)))
            })
    }

    /// The mutability [`Self::origin_mutability_slot`] decides for an
    /// origin `origin`: the origin's own when it alone decides it (a
    /// cast's, a static origin's), else `bound`, else the origin's own
    /// through its owner or enclosing binder.
    fn decided_slot_mutability(
        &self,
        origin: &Origin,
        bound: Option<Mutability>,
    ) -> Option<Mutability> {
        origin
            .static_mutability()
            .map(Mutability::of)
            .or(bound)
            .or_else(|| self.decided_origin_mutability(origin))
    }

    /// The value a call records in its callee's `mut=` binder `decl`
    /// governing `origin`: [`Self::origin_mutability_slot`]'s, except that
    /// a mutability an implicit binder of the caller decides with no
    /// `mut=` binder in scope is that binder's `$origin_mut` read
    /// ([`Mutability::mut_argument`]), as upstream's call reads the
    /// caller's implicit `self_is_mut`, rather than the callee's own
    /// binder, which no caller binds. A type keeps the callee's binder, so
    /// it still compares equal to the declared type it instantiates.
    fn call_mutability_slot(
        &self,
        decl: &ParamDecl,
        origin: &Origin,
        bound: Option<Mutability>,
    ) -> CtValue {
        match self.origin_mutability_slot(decl, origin, bound) {
            CtValue::Deferred(_)
                if let Some(Mutability::Param(id)) =
                    self.decided_slot_mutability(origin, bound) =>
            {
                Mutability::Param(id).mut_argument()
            }
            value => value,
        }
    }

    /// Bind the struct binders one declared parameter or field type names
    /// from the actual filling it: a pointer's or `!lit.ref`'s binder from
    /// the actual pointer's or `!lit.ref`'s provenance, a reference's binder from the reference's origin, and a
    /// struct-typed pattern's tail binders from the actual's tail (recursing
    /// through nested type arguments and tuples).
    #[allow(clippy::too_many_arguments)]
    fn bind_origin_pattern(
        struct_name: &str,
        context: &dyn Fn() -> String,
        params: &[ParamDecl],
        pattern: &Ty,
        actual: &Ty,
        explicit: SuppliedOrigins<'_>,
        bindings: &mut ConstructorOriginBindings,
    ) -> Result<(), TypeError> {
        let slot_of = |id: &OriginParamId| super::super::struct_origin_decl(params, *id);
        match (pattern.kind(), actual.kind()) {
            (pattern_kind, actual_kind)
                if let Some((pattern, actual)) =
                    pattern_kind.matching_pointer_parts(actual_kind)
                    && let Some((pattern, _)) =
                        pattern.origin.canonical_pointer_parts(pattern.mutability)
                    && let Some((id, projection)) = pattern.binder_projection()
                    && let Some(actual_origin) =
                        actual.origin.canonical_pointer_parts(actual.mutability) =>
            {
                let Some(slot) = slot_of(&id) else {
                    return Ok(());
                };
                let requires_mut = matches!(
                    slot,
                    ParamDecl::Origin {
                        mutability: mojito_types::types::OriginMutability::Fixed(true),
                        ..
                    }
                );
                if requires_mut && let Some(found) = not_statically_mutable(&actual_origin) {
                    return Err(TypeError::TypeMismatch {
                        expected: format!(
                            "a mutable-origin pointer for parameter '{}' of '{struct_name}'",
                            slot.name()
                        ),
                        found,
                        context: context(),
                    });
                }
                if let Some(supplied) = explicit.find(id) {
                    let actual = &actual_origin.0;
                    let within = if actual.pointer_loan_origin().is_some() {
                        explicit.admits(actual, &supplied.origin)
                    } else {
                        supplied.origin.is_untracked()
                            || (*actual == Origin::Static && supplied.origin == Origin::Static)
                    };
                    if !within {
                        return Err(TypeError::TypeMismatch {
                            expected: format!(
                                "a pointer whose origin is the supplied '{}' argument",
                                slot.name()
                            ),
                            found: format!("a pointer of origin {actual}"),
                            context: context(),
                        });
                    }
                }
                // A projected parameter origin (`Self.origin._subtree`)
                // binds the provenance below the projection; one the actual
                // does not end in is checked by coercion, not bound.
                let (actual, mutability) = actual_origin;
                actual
                    .without_projection(projection)
                    .map_or(Ok(()), |base| {
                        bindings.bind_pointer(id, (base, mutability), || {
                            conflicting_binding(struct_name, slot.name())
                        })
                    })
            }
            (TyKind::Ref(declared), TyKind::Ref(reference)) => {
                let Origin::Param(id) = &declared.origin else {
                    return Ok(());
                };
                let Some(slot) = slot_of(id) else {
                    return Ok(());
                };
                if let Some(supplied) = explicit.find(*id)
                    && !explicit.admits(&reference.origin, &supplied.origin)
                {
                    return Err(TypeError::TypeMismatch {
                        expected: format!(
                            "a reference whose origin is the supplied '{}' argument",
                            slot.name()
                        ),
                        found: format!("a reference of origin {}", reference.origin),
                        context: context(),
                    });
                }
                bindings.bind_origin(*id, &reference.origin, || {
                    conflicting_binding(struct_name, slot.name())
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
                            params,
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
                            if let Some(supplied) = explicit.find(*id)
                                && !matches!(actual, Origin::Unbound)
                                && !explicit.admits(actual, &supplied.origin)
                            {
                                return Err(TypeError::TypeMismatch {
                                    expected: format!(
                                        "a value whose origin is the supplied '{}' argument",
                                        slot.name()
                                    ),
                                    found: format!("a value of origin {actual}"),
                                    context: context(),
                                });
                            }
                            bindings.bind_origin(*id, actual, || {
                                conflicting_binding(struct_name, slot.name())
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
                        params,
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
    names_origin_binder_in(parameter, &|_| true)
}

/// [`names_origin_binder`] for the binders `among` admits.
pub(in crate::checker) fn names_origin_binder_in(
    parameter: &Ty,
    among: &dyn Fn(OriginParamId) -> bool,
) -> bool {
    mojito_types::types::mentions(parameter, &|candidate| match candidate.kind() {
        handle if let Some(reference) = handle.pointer_or_lit_ref() => matches!(
            reference.origin.uncast().projection_base(),
            Origin::Param(id) if among(*id)
        ),
        TyKind::Ref(reference) => matches!(reference.origin, Origin::Param(id) if among(id)),
        struct_ty if let Some((_, arguments)) = struct_ty.as_non_pointer_struct() => arguments
            .iter()
            .any(|argument| matches!(argument, TyArg::Origin(Origin::Param(id)) if among(*id))),
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
            if let Some((declared, provenance)) =
                pattern_kind.matching_pointer_parts(actual_kind) =>
        {
            if let Some((pattern, _)) = declared.origin.canonical_pointer_parts(declared.mutability)
                && let Some((id, projection)) = pattern.binder_projection()
                && let Some((actual, mutability)) = provenance
                    .origin
                    .canonical_pointer_parts(provenance.mutability)
                && let Some(base) = actual.without_projection(projection)
            {
                bindings.bind_pointer(id, (base, mutability), conflict)?;
            }
            bind_callable_tail_pattern(&declared.referent, &provenance.referent, bindings, conflict)
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
                // A `ref[o]` argument over `o: Origin[mut=m]` lends `m` its
                // capability, as upstream's `!lit.ref` binds both.
                if declared.mutability == Mutability::Param(*id) && bindings.binders.binds(*id) {
                    bindings
                        .references
                        .entry(*id)
                        .or_insert(reference.mutability);
                }
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
        Origin::Place(_) | Origin::Projected(..) => origin.without_subtree(),
        Origin::Union(members) => Origin::union(members.into_iter().map(tail_origin)),
        Origin::MutCast(inner, mutability) => Origin::mut_cast(tail_origin(*inner), mutability),
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
        Origin::Untracked { mutable } | Origin::UnsafeAny { mutable } => Some(if *mutable {
            Mutability::Mutable
        } else {
            Mutability::Immutable
        }),
        Origin::Static => Some(Mutability::Immutable),
        Origin::MutCast(..) => origin.static_mutability().map(Mutability::of),
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

fn conflicting_binding(struct_name: &str, slot: &str) -> TypeError {
    TypeError::BadCall {
        func: struct_name.to_string(),
        reason: format!("arguments bind conflicting origins for parameter '{slot}'"),
    }
}

/// Why a pointer of provenance `origin` does not pass a `MutOrigin` binder,
/// `None` when it does: an immutable origin, or a callable's own origin
/// binder whose mutability is itself a parameter, which upstream converts to
/// `MutOrigin` no more than an immutable one. The receiver's symbolic
/// `origin_of(self)` still passes (R1215).
fn not_statically_mutable((origin, mutability): &(Origin, Mutability)) -> Option<String> {
    match mutability.known() {
        Some(false) => Some("an immutable-origin pointer".to_string()),
        None if origin.binder_projection().is_some() => {
            Some("a pointer whose origin's mutability is a parameter".to_string())
        }
        _ => None,
    }
}

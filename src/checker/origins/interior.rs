//! Origin-signature validation plus interior/aggregate origin
//! recording and invalidation bookkeeping.

use super::*;

impl Checker {
    pub(in crate::checker) fn validate_origin_signature(
        &self,
        type_params: &[crate::ast::TypeParam],
        params: &[crate::ast::FnParam],
        self_origin: Option<&crate::ast::OriginSpec>,
    ) -> Result<(), TypeError> {
        let origin_params: HashSet<&str> = type_params
            .iter()
            .filter(|param| param.bounds.as_slice() == ["Origin"])
            .map(|param| param.name.as_str())
            .collect();
        let value_params: HashSet<&str> = params.iter().map(|param| param.name.as_str()).collect();
        let bool_params: HashSet<&str> = type_params
            .iter()
            .filter(|param| param.bounds.as_slice() == ["Bool"])
            .map(|param| param.name.as_str())
            .collect();

        for origin in type_params
            .iter()
            .filter(|param| param.bounds.as_slice() == ["Origin"])
        {
            if let Some(expr) = &origin.origin_mutability
                && !matches!(expr.kind, ExprKind::Bool(_))
                && !matches!(&expr.kind, ExprKind::Identifier(name) if bool_params.contains(name.as_str()))
            {
                return Err(TypeError::Unsupported(format!(
                    "origin mutability for '{}' must be Bool or a Bool parameter",
                    origin.name
                )));
            }
        }

        let validate = |spec: &crate::ast::OriginSpec| {
            for expr in spec {
                validate_origin_expr(expr, &origin_params, &value_params)?;
            }
            Ok::<(), TypeError>(())
        };
        if let Some(spec) = self_origin {
            validate(spec)?;
        }
        for param in params {
            if param.convention != Some(ArgConvention::Ref) && param.origin.is_some() {
                return Err(TypeError::Unsupported(format!(
                    "origin clause on non-ref parameter '{}'",
                    param.name
                )));
            }
            if let Some(spec) = &param.origin {
                validate(spec)?;
            }
        }
        Ok(())
    }

    /// Record that evaluating `expression` as a reference produces a fresh
    /// generation in the named interior region owned by `base`.
    pub(in crate::checker) fn record_interior_reference(
        &self,
        site: SourceSpan,
        base: &Expr,
        name: &str,
    ) {
        if self
            .interior_references
            .borrow()
            .get(&site)
            .is_some_and(|origin| {
                matches!(origin.path.last(), Some(crate::origin::OriginSeg::Interior(tag)) if tag == name)
            })
        {
            // Inference is intentionally repeatable. Do not project the fact
            // through itself when the same checked expression is revisited.
            return;
        }
        if let Ok(mut origin) = self.origin_place(base) {
            origin
                .path
                .push(crate::origin::OriginSeg::Interior(name.to_string()));
            self.interior_references.borrow_mut().insert(site, origin);
        }
    }

    /// Record Mojo's owned-interior generation refresh for a named region.
    /// Defining a new `base._get_owned_interior["name"]` origin invalidates an
    /// older generation of that same region, but not sibling regions below the
    /// owner. Dict lookup uses this for `"value"`, so a new lookup stales an
    /// earlier value reference without invalidating the `"element"` generation
    /// retained by key iteration.
    pub(in crate::checker) fn record_replacing_interior_reference(
        &self,
        site: SourceSpan,
        base: &Expr,
        name: &str,
    ) {
        if let Ok(mut origin) = self.origin_place(base) {
            origin
                .path
                .push(crate::origin::OriginSeg::Interior(name.to_string()));
            self.record_origin_invalidation_kind(site.clone(), origin, None, true);
        }
        self.record_interior_reference(site, base, name);
    }

    /// Record a mutation of `base`. Existing generations rooted below this
    /// path become stale. If `base` is itself a local reference, mutations
    /// through that handle preserve its own generation while still invalidating
    /// interiors nested underneath it.
    pub(in crate::checker) fn record_interior_invalidation(&self, site: SourceSpan, base: &Expr) {
        let Ok(origin) = self.origin_place(base) else {
            return;
        };
        let except = match &base.kind {
            ExprKind::Identifier(name) if matches!(self.lookup(name), Some(Ty::Ref(_))) => {
                self.lookup_owner(name)
            }
            _ => None,
        };
        self.record_origin_invalidation(site, origin, except);
    }

    /// Record the storage generation replaced by a checked place write. Index
    /// and Variant-payload targets are places too: replacing one preserves a
    /// reference to that exact generation, but invalidates references into
    /// interiors nested below it. Two handle-bearing place forms need their
    /// semantic referent rather than their syntactic storage path:
    ///
    /// * `pointer[0]` replaces the origin-bearing pointer's proven source place;
    /// * assigning through a reference-valued aggregate field replaces the
    ///   place(s) whose handles the aggregate retains.
    pub(in crate::checker) fn record_place_write_invalidation(
        &self,
        site: SourceSpan,
        place: &Expr,
    ) {
        if let ExprKind::Index { object, .. } = &place.kind
            && let Ok(Ty::Pointer {
                origin: crate::origin::PointerOrigin::Place { place: origin, .. },
                ..
            }) = self.infer(object)
        {
            self.record_origin_invalidation(site, origin, None);
            return;
        }

        if matches!(self.place_storage_ty(place), Some(Ty::Ref(_))) {
            // An `out self` initializer stores the incoming reference handle;
            // subsequent assignments write through that established handle.
            if self.self_initializing && place_root_name(place) == Some("self") {
                return;
            }

            let mut origins = self.aggregate_origins(place).into_iter().peekable();
            if origins.peek().is_some() {
                for origin in origins {
                    self.record_aggregate_origin_invalidation(site.clone(), origin);
                }
                return;
            }
        }

        self.record_interior_invalidation(site, place);
    }

    pub(in crate::checker) fn record_aggregate_origin_invalidation(
        &self,
        site: SourceSpan,
        origin: crate::origin::Origin,
    ) {
        self.record_aggregate_origin_invalidation_except(site, origin, None);
    }

    pub(in crate::checker) fn record_aggregate_origin_invalidation_except(
        &self,
        site: SourceSpan,
        origin: crate::origin::Origin,
        except: Option<crate::origin::OwnerId>,
    ) {
        match origin {
            crate::origin::Origin::Place(place) => {
                self.record_origin_invalidation(site, place, except);
            }
            crate::origin::Origin::Union(members) => {
                for member in members {
                    self.record_aggregate_origin_invalidation_except(site.clone(), member, except);
                }
            }
            crate::origin::Origin::Param(_)
            | crate::origin::Origin::SelfParam
            | crate::origin::Origin::Static
            | crate::origin::Origin::Untracked { .. } => {}
        }
    }

    pub(in crate::checker) fn record_origin_invalidation(
        &self,
        site: SourceSpan,
        base: crate::origin::OriginPlace,
        except: Option<crate::origin::OwnerId>,
    ) {
        self.record_origin_invalidation_kind(site, base, except, false);
    }

    pub(in crate::checker) fn record_origin_invalidation_kind(
        &self,
        site: SourceSpan,
        base: crate::origin::OriginPlace,
        except: Option<crate::origin::OwnerId>,
        include_base_generation: bool,
    ) {
        let fact = crate::checked::InteriorInvalidation {
            base,
            except,
            include_base_generation,
        };
        let mut invalidations = self.interior_invalidations.borrow_mut();
        let values = invalidations.entry(site).or_default();
        if !values.contains(&fact) {
            values.push(fact);
        }
    }

    pub(in crate::checker) fn record_owner_invalidation(
        &self,
        site: SourceSpan,
        owner: crate::origin::OwnerId,
        path: Vec<crate::origin::OriginSeg>,
    ) {
        let fact = crate::checked::InteriorInvalidation {
            base: crate::origin::OriginPlace { root: owner, path },
            except: None,
            include_base_generation: false,
        };
        let mut invalidations = self.interior_invalidations.borrow_mut();
        let values = invalidations.entry(site).or_default();
        if !values.contains(&fact) {
            values.push(fact);
        }
    }

    pub(in crate::checker) fn lookup_aggregate_origins(
        &self,
        name: &str,
    ) -> Vec<crate::origin::Origin> {
        let mut origins = self.lookup_scoped_aggregate_origins(name);
        // Merge origins a callee's store transferred into this binding.
        if let Some(owner) = self.lookup_owner(name)
            && let Some(transferred) = self.transferred_origins.borrow().get(&owner)
        {
            for origin in transferred {
                if !origins.contains(origin) {
                    origins.push(origin.clone());
                }
            }
        }
        origins
    }

    pub(super) fn lookup_scoped_aggregate_origins(&self, name: &str) -> Vec<crate::origin::Origin> {
        self.aggregate_origin_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
            .unwrap_or_default()
    }

    pub(in crate::checker) fn set_aggregate_origins(
        &mut self,
        name: &str,
        origins: Vec<crate::origin::Origin>,
    ) {
        let Some(scope) = self.binding_scope(name) else {
            return;
        };
        if origins.is_empty() {
            self.aggregate_origin_scopes[scope].remove(name);
        } else {
            self.aggregate_origin_scopes[scope].insert(name.to_string(), origins);
        }
    }

    pub(in crate::checker) fn lookup_aggregate_field_origins(
        &self,
        name: &str,
    ) -> HashMap<String, Vec<crate::origin::Origin>> {
        self.aggregate_field_origin_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
            .unwrap_or_default()
    }

    pub(in crate::checker) fn set_aggregate_field_origins(
        &mut self,
        name: &str,
        fields: HashMap<String, Vec<crate::origin::Origin>>,
    ) {
        let Some(scope) = self.binding_scope(name) else {
            return;
        };
        if fields.is_empty() {
            self.aggregate_field_origin_scopes[scope].remove(name);
        } else {
            self.aggregate_field_origin_scopes[scope].insert(name.to_string(), fields);
        }
    }

    /// Origins retained by each direct field of an aggregate value. The flat
    /// aggregate origin set remains useful for lifetime extension, but it
    /// cannot identify the referent of `pair.right` when `pair` also retains a
    /// distinct `left` origin.
    pub(in crate::checker) fn aggregate_field_origins(
        &self,
        expression: &Expr,
    ) -> HashMap<String, Vec<crate::origin::Origin>> {
        fn append_unique(
            into: &mut Vec<crate::origin::Origin>,
            values: impl IntoIterator<Item = crate::origin::Origin>,
        ) {
            for value in values {
                if !into.contains(&value) {
                    into.push(value);
                }
            }
        }

        match &expression.kind {
            ExprKind::Identifier(name) => self.lookup_aggregate_field_origins(name),
            ExprKind::Transfer(inner) | ExprKind::Named { value: inner, .. } => {
                self.aggregate_field_origins(inner)
            }
            ExprKind::IfExpr {
                then_branch,
                else_branch,
                ..
            } => {
                let mut result = self.aggregate_field_origins(then_branch);
                for (field, origins) in self.aggregate_field_origins(else_branch) {
                    append_unique(result.entry(field).or_default(), origins);
                }
                result
            }
            ExprKind::Call {
                name, args, kwargs, ..
            } => {
                let Some(info) = self.structs.get(name) else {
                    return HashMap::new();
                };
                let fields = info.fields.clone();
                let mut result = HashMap::new();
                if info.fieldwise_init {
                    for ((field_name, field_ty), argument) in fields.into_iter().zip(args) {
                        let origins = if matches!(field_ty, Ty::Ref(_)) {
                            // An owned place auto-borrows into a ref field;
                            // synthesize its place origin like the equivalent
                            // `ref` binding would.
                            self.infer_reference_value(argument)
                                .or_else(|| self.reference_actual(argument).ok())
                                .map(|reference| vec![reference.origin])
                                .unwrap_or_default()
                        } else if self.type_may_carry_loans(&field_ty) {
                            self.aggregate_origins(argument)
                        } else {
                            Vec::new()
                        };
                        if !origins.is_empty() {
                            result.insert(field_name, origins);
                        }
                    }
                    return result;
                }

                // A conventional handwritten initializer commonly forwards a
                // same-named ref parameter into each reference field. Preserve
                // that field identity at the call site too; arbitrary computed
                // initializer data flow remains represented by the flat,
                // conservative aggregate origin set.
                let Some(signature) = info.methods.get("__init__").and_then(|signatures| {
                    signatures
                        .iter()
                        .find(|signature| signature.params.len() >= args.len())
                }) else {
                    return result;
                };
                for (field_name, field_ty) in fields {
                    // A `ref[o]` field, or upstream's `Pointer[T, Self.o]`
                    // storage of a forwarded `ref[Self.o]` parameter.
                    let origin_bearing = matches!(field_ty, Ty::Ref(_))
                        || matches!(
                            field_ty,
                            Ty::Pointer {
                                origin: crate::origin::PointerOrigin::Param { .. },
                                ..
                            }
                        );
                    if !origin_bearing {
                        continue;
                    }
                    let Some(index) = signature
                        .names
                        .iter()
                        .position(|parameter| parameter == &field_name)
                    else {
                        continue;
                    };
                    let argument = args.get(index).or_else(|| {
                        kwargs
                            .iter()
                            .find(|argument| argument.name == field_name)
                            .map(|argument| &argument.value)
                    });
                    if let Some(argument) = argument {
                        let origins = self
                            .reference_actual(argument)
                            .ok()
                            .map(|reference| vec![reference.origin])
                            .or_else(|| {
                                signature
                                    .ref_params
                                    .get(index)
                                    .is_some_and(Option::is_some)
                                    .then(|| {
                                        self.origin_place(argument)
                                            .ok()
                                            .map(crate::origin::Origin::Place)
                                            .into_iter()
                                            .collect::<Vec<_>>()
                                    })
                            })
                            .unwrap_or_default();
                        if !origins.is_empty() {
                            result.insert(field_name, origins);
                        }
                    }
                }
                result
            }
            _ => HashMap::new(),
        }
    }
}

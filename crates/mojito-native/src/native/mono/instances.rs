//! Derived-instance enqueueing: hash leaves, nominal methods,
//! display, and Intable instances.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl Specializer<'_> {
    /// The constant a value-parameter member read (`Self.length`) resolves
    /// to, when `field` names a value parameter (not a declared field) of
    /// the receiver's template and the instance type carries its solution.
    pub(super) fn value_param_constant(&self, receiver: &Ty, field: &str) -> Option<Const> {
        let Ty::Struct(name, type_args) = peel_refs(receiver) else {
            return None;
        };
        let struct_decl = self.structs.get(nominal_template(name)).copied()?;
        if struct_decl
            .fields
            .iter()
            .any(|(field_name, _)| field_name == field)
        {
            return None;
        }
        let position = struct_decl
            .param_decls
            .iter()
            .position(|decl| matches!(decl, ParamDecl::Value { name, .. } if name == field))?;
        let TyArg::Val(value) = type_args.get(position)? else {
            return None;
        };
        match value {
            CtValue::Int(v) => Some(Const::Int(*v)),
            CtValue::UInt(v) => Some(Const::Int(*v as i64)),
            CtValue::Bool(v) => Some(Const::Bool(*v)),
            CtValue::Dtype(v) => Some(Const::Dtype(*v)),
            value @ (CtValue::Simd { .. } | CtValue::Struct { .. }) => {
                Some(Const::Value(value.clone()))
            }
            _ => None,
        }
    }

    /// Enqueue the instances a lowered scalar `__hash__(hasher)` leaf calls:
    /// for a scalar or vector receiver, the hasher's `_update_with_simd`
    /// instantiated at the leaf's own vector type (`hash_leaf_ty`), its
    /// dtype and width binders solved from that type as a call's argument
    /// solves them. A string-literal
    /// receiver reaches the hasher through the nominal String's `__hash__`
    /// bound to it (the VM materializes the literal and dispatches the same
    /// way), and a Variant receiver through the `__hash__` of every nominal
    /// alternative (the lowered tag switch dispatches whichever is active).
    pub(super) fn enqueue_hash_leaf_instances(
        &mut self,
        owner: &str,
        function: &MirFunction,
        hasher: Reg,
        receiver: &Ty,
    ) -> Result<(), MonoError> {
        let Some(hasher_ty) = function.reg_types.get(&hasher.0) else {
            return Ok(());
        };
        let hasher_ty = peel_refs(hasher_ty).clone();
        if !matches!(hasher_ty, Ty::Struct(..)) {
            return Ok(());
        }
        if mojito_types::types::simd_shape(receiver).is_some()
            || matches!(receiver, Ty::Bool | Ty::Dtype)
        {
            let leaf = mojito_types::types::hash_leaf_ty(receiver);
            self.nominal_method_instance(
                owner,
                &hasher_ty,
                "_update_with_simd",
                1,
                &[],
                std::slice::from_ref(&leaf),
            )?;
            return Ok(());
        }
        if matches!(receiver, Ty::StringLiteral) {
            let string = Ty::Struct(
                mojito_symbol::symbol::STDLIB_STRING_STRUCT.to_string(),
                Vec::new().into(),
            );
            self.enqueue_nominal_method_instance(
                owner,
                &string,
                "__hash__",
                1,
                std::slice::from_ref(&hasher_ty),
            )?;
        }
        if let Ty::Variant(alternatives) = receiver {
            for alternative in alternatives {
                if matches!(alternative, Ty::Struct(..)) {
                    self.enqueue_nominal_method_instance(
                        owner,
                        alternative,
                        "__hash__",
                        1,
                        std::slice::from_ref(&hasher_ty),
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Enqueue one nominal method instance reached by a lowered intrinsic
    /// rather than an explicit MIR call: the receiver binds the owner's
    /// parameters, `method_bindings` bind the method's own type parameters in
    /// declaration order, and any remaining type parameter (a `Some[..]`
    /// sugar spelling) instantiates at the builtin string, as
    /// `enqueue_display_instance`.
    pub(super) fn enqueue_nominal_method_instance(
        &mut self,
        owner: &str,
        receiver: &Ty,
        method: &str,
        argc: usize,
        method_bindings: &[Ty],
    ) -> Result<(), MonoError> {
        self.nominal_method_instance(owner, receiver, method, argc, method_bindings, &[])
            .map(drop)
    }

    /// Enqueue the `write_to` instance a lowered `print` of a nominal struct
    /// calls — the VM's `format_value` dispatch. The receiver binds the
    /// owner's parameters; the writer parameter instantiates at the builtin
    /// string (the VM's `Value::Str` accumulator).
    /// The per-instantiation clone a by-name dunder rewrite dispatches on a
    /// receiver whose checked static type is a closed generic-struct
    /// instance (`Box.__len__$y3:Int` for a `Box[Int]` register), when the
    /// checker minted it; `None` keeps the template's method, as the VM's
    /// runtime-name dispatch does for every erased body.
    pub(super) fn instance_dunder_target(
        &self,
        function: &MirFunction,
        receiver: Reg,
        method: &str,
    ) -> Option<String> {
        let Ty::Struct(name, arguments) = peel_refs(function.reg_types.get(&receiver.0)?) else {
            return None;
        };
        let template = nominal_template(name);
        let clone = self.instance_method_clone(template, method, arguments)?;
        let target = format!("{template}.{clone}");
        self.functions
            .contains_key(target.as_str())
            .then_some(target)
    }

    /// The per-instantiation clone that serves a call of the template method
    /// `target` on `receiver`, when the checker minted one for the
    /// receiver's instance: a method holding a compile-time construct keeps
    /// its clones, and a template body that calls it names the template.
    pub(super) fn instance_method_target(
        &self,
        function: &MirFunction,
        receiver: Reg,
        target: &str,
        argc: usize,
    ) -> Option<String> {
        let Ty::Struct(name, arguments) = peel_refs(function.reg_types.get(&receiver.0)?) else {
            return None;
        };
        let template = nominal_template(name);
        let method = target.strip_prefix(template)?.strip_prefix('.')?;
        if mojito_symbol::symbol::specialization_template(method).is_some() {
            return None;
        }
        let source = mojito_symbol::symbol::overload_qualifier(method)
            .map_or(method, |qualifier| {
                &method[..method.len() - qualifier.len()]
            });
        let clone = self.instance_method_clone(template, source, arguments)?;
        let selected = mojito_symbol::symbol::resolve_method_symbol(
            self.functions.iter().map(|(name, f)| CallableCandidate {
                name,
                n_params: f.n_params,
            }),
            template,
            &clone,
            None,
            argc,
        );
        self.functions
            .contains_key(selected.as_str())
            .then_some(selected)
    }

    /// The symbol the checker mints a per-instantiation clone of
    /// `template`'s `method` under for the instance over `arguments`
    /// (`__len__$y3:Int` for `Box[Int]`), or `None` when it minted no clone
    /// of the method at all. Spelling the arguments walks every level of a
    /// nested type, so the second answer comes first.
    pub(super) fn instance_method_clone(
        &self,
        template: &str,
        method: &str,
        arguments: &[TyArg],
    ) -> Option<String> {
        let clone_prefix = format!("{template}.{method}$");
        self.function_names
            .range(clone_prefix.as_str()..)
            .next()
            .filter(|name| name.starts_with(&clone_prefix))?;
        mojito_symbol::symbol::instance_method_clone_name(
            method,
            &self.structs.get(template)?.param_decls,
            &super::specializer::template_spelled_arguments(arguments),
        )
    }

    /// The receiver's own overload a bound dispatch `dispatch` selects
    /// (`__trait_dispatch.__hash__$ov$Some$u5B$Hasher$u5D$$Hasher` on
    /// `Twin`), when no lowered symbol spells its qualifier: the requirement
    /// spells its parameter as the trait declares it, and the witness may
    /// bind it under a name of its own (`Twin.__hash__$ov$H$Hasher`). The
    /// declaration whose parameter types key as the requirement's is the
    /// one.
    pub(super) fn dispatched_overload_target(
        &self,
        template: &str,
        method: &str,
        dispatch: &str,
    ) -> Option<String> {
        let qualifier = mojito_symbol::symbol::overload_qualifier(dispatch)?;
        let base = format!("{template}.{method}");
        self.functions
            .keys()
            .filter(|name| mojito_symbol::symbol::is_overload_of(name, &base))
            .find(|name| {
                let Some(declaration) = self.declarations.get(*name) else {
                    return false;
                };
                // The declaration lists no receiver; the function's first
                // parameter is the struct type its `Self`-typed parameters
                // canonicalize to.
                let self_ty = declaration
                    .has_receiver
                    .then(|| self.functions.get(*name)?.param_types.first())
                    .flatten()
                    .map(peel_refs);
                let keywords = declaration
                    .keyword_only
                    .map(|index| declaration.param_names[index..].to_vec())
                    .unwrap_or_default();
                // A builtin trait's requirement spells its one-bound
                // parameter existentially (`hasher: Some[Hasher]`), which a
                // witness binding it by name (`[H: Hasher](hasher: H)`)
                // satisfies alike.
                let existential = declaration
                    .param_types
                    .iter()
                    .map(existential_spelling)
                    .collect::<Vec<_>>();
                [declaration.param_types.as_slice(), existential.as_slice()]
                    .into_iter()
                    .any(|params| {
                        let signature = mojito_symbol::symbol::SignatureKey::from_tys_with_self(
                            params, self_ty,
                        )
                        .with_kw_variadic(declaration.kw_variadic.as_ref())
                        .with_keyword_names(keywords.clone());
                        let spelled = if mojito_symbol::symbol::receiver_overloaded_method(method) {
                            if declaration.has_receiver {
                                mojito_symbol::symbol::receiver_method_symbol(
                                    template,
                                    method,
                                    declaration.receiver_convention,
                                    &signature,
                                )
                            } else {
                                mojito_symbol::symbol::static_method_symbol(
                                    template, method, &signature,
                                )
                            }
                        } else {
                            mojito_symbol::symbol::method_symbol(template, method, &signature)
                        };
                        mojito_symbol::symbol::overload_qualifier(&spelled) == Some(qualifier)
                    })
            })
            .map(|name| (*name).to_string())
    }

    pub(super) fn enqueue_display_instance(
        &mut self,
        owner: &str,
        function: &MirFunction,
        arg: Reg,
    ) -> Result<(), MonoError> {
        self.enqueue_format_instance(owner, function, arg, "write_to")
    }

    pub(super) fn enqueue_repr_instance(
        &mut self,
        owner: &str,
        function: &MirFunction,
        arg: Reg,
    ) -> Result<(), MonoError> {
        self.enqueue_format_instance(owner, function, arg, "write_repr_to")
    }

    fn enqueue_format_instance(
        &mut self,
        owner: &str,
        function: &MirFunction,
        arg: Reg,
        method: &str,
    ) -> Result<(), MonoError> {
        let Some(ty) = function.reg_types.get(&arg.0) else {
            return Ok(());
        };
        let ty = peel_refs(ty).clone();
        let Ty::Struct(name, arguments) = &ty else {
            return Ok(());
        };
        let exact = format!("{name}.{method}");
        if self.functions.contains_key(exact.as_str()) {
            if self.writes_protocol(&exact) {
                self.enqueue_nominal_method_instance(owner, &ty, method, 1, &[])?;
            }
            return Ok(());
        }
        // A closed instance displays through its per-instantiation
        // `write_to` clone when the checker minted one; otherwise the
        // template's erased `write_to` is instantiated for the receiver.
        let clone_target = self
            .instance_method_clone(nominal_template(name), method, arguments)
            .map(|clone| format!("{}.{clone}", nominal_template(name)))
            .filter(|target| self.functions.contains_key(target.as_str()));
        let target = clone_target.unwrap_or_else(|| {
            mojito_symbol::symbol::resolve_method_symbol(
                self.functions.iter().map(|(name, f)| CallableCandidate {
                    name,
                    n_params: f.n_params,
                }),
                nominal_template(name),
                method,
                None,
                1,
            )
        });
        // Beside a rival of the same arity (`write_to[U](self, mut writer:
        // List[U])`), no symbol answers by arity; the overload whose first
        // parameter is a `Writer` is the protocol's witness.
        let target = if self.functions.contains_key(target.as_str()) {
            target
        } else {
            self.protocol_overload(nominal_template(name), method)
                .unwrap_or(target)
        };
        if !self.functions.contains_key(target.as_str()) || !self.writes_protocol(&target) {
            return Ok(());
        }
        let Some(declaration) = self.declarations.get(target.as_str()).copied() else {
            return Ok(());
        };
        let mut bindings = self.base_bindings();
        let mut owner_covered = 0;
        if let Some(struct_decl) = self.structs.get(nominal_template(name)).copied() {
            bind_ty_args(&struct_decl.param_decls, arguments, &mut bindings).map_err(|e| {
                self.error(
                    Some(owner),
                    format!("monomorphizing receiver for `{target}`: {e}"),
                )
            })?;
            if nominal_template(name) != name.as_str() {
                bindings.self_instance = Some((nominal_template(name).to_string(), ty.clone()));
                owner_covered =
                    owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
            }
        }
        for decl in &declaration.param_decls {
            if let ParamDecl::Type { .. } = decl
                && !bindings.types.contains_key(&decl.binder())
            {
                bindings.types.insert(decl.binder(), Ty::StringLiteral);
            }
        }
        // The `Some[Writer]` sugar parameter is infer-only (absent from
        // `param_decls`); bind its spelling from the declared type.
        for ty in &declaration.param_types {
            if let Ty::Param { binder, .. } = ty
                && !bindings.types.contains_key(binder)
            {
                bindings.types.insert(binder.clone(), Ty::StringLiteral);
            }
        }
        let mut arguments = ordered_arguments(&declaration.param_decls, &bindings, &target)?;
        arguments.drain(..owner_covered);
        self.enqueue(&target, bindings, arguments)?;
        Ok(())
    }

    /// Enqueue a nominal `__int__` reached implicitly by scalar/SIMD
    /// construction. The checker records the concrete operand type on the
    /// element register, while MIR deliberately keeps construction as
    /// `MakeSimd` rather than synthesizing a method call.
    pub(super) fn enqueue_intable_instance(
        &mut self,
        owner: &str,
        function: &MirFunction,
        arg: Reg,
    ) -> Result<(), MonoError> {
        let Some(ty @ Ty::Struct(name, _)) = function.reg_types.get(&arg.0) else {
            return Ok(());
        };
        let target = mojito_symbol::symbol::resolve_method_symbol(
            self.functions.iter().map(|(name, f)| CallableCandidate {
                name,
                n_params: f.n_params,
            }),
            nominal_template(name),
            "__int__",
            None,
            0,
        );
        if !self.functions.contains_key(target.as_str()) {
            return Ok(());
        }
        let (bindings, arguments, _) = self.infer_receiver_call(owner, &target, ty, None, &[])?;
        self.enqueue(&target, bindings, arguments)?;
        Ok(())
    }

    /// Whether a display method takes the `Writable` protocol's writer; any
    /// other `write_to` is an ordinary overload and never serves display.
    /// The one overload of `template.method` whose first parameter is a
    /// `Writer`.
    fn protocol_overload(&self, template: &str, method: &str) -> Option<String> {
        let base = format!("{template}.{method}");
        let mut overloads = self.declarations.iter().filter_map(|(name, declaration)| {
            (mojito_symbol::symbol::is_overload_of(name, &base)
                && declaration
                    .param_types
                    .first()
                    .is_some_and(mojito_types::types::is_writer_parameter))
            .then_some(name)
        });
        let overload = overloads.next()?;
        overloads.next().is_none().then(|| overload.to_string())
    }

    fn writes_protocol(&self, target: &str) -> bool {
        self.declarations.get(target).is_none_or(|declaration| {
            declaration
                .param_types
                .first()
                .is_some_and(mojito_types::types::is_writer_parameter)
        })
    }

    /// The instance one nominal method reached by a lowered intrinsic
    /// names, enqueued (see [`Self::enqueue_nominal_method_instance`]):
    /// `argument_types`, when given, solve the method's own binders as a
    /// call's arguments would before any remaining type parameter takes
    /// the builtin string. `None` when the receiver declares no such method.
    fn nominal_method_instance(
        &mut self,
        owner: &str,
        receiver: &Ty,
        method: &str,
        argc: usize,
        method_bindings: &[Ty],
        argument_types: &[Ty],
    ) -> Result<Option<String>, MonoError> {
        let Ty::Struct(name, arguments) = receiver else {
            return Ok(None);
        };
        let target = mojito_symbol::symbol::resolve_method_symbol(
            self.functions.iter().map(|(name, f)| CallableCandidate {
                name,
                n_params: f.n_params,
            }),
            nominal_template(name),
            method,
            None,
            argc,
        );
        if !self.functions.contains_key(target.as_str()) {
            return Ok(None);
        }
        let Some(declaration) = self.declarations.get(target.as_str()).copied() else {
            return Ok(None);
        };
        let mut bindings = self.base_bindings();
        let mut owner_covered = 0;
        if let Some(struct_decl) = self.structs.get(nominal_template(name)).copied() {
            bind_ty_args(&struct_decl.param_decls, arguments, &mut bindings).map_err(|e| {
                self.error(
                    Some(owner),
                    format!("monomorphizing receiver for `{target}`: {e}"),
                )
            })?;
            if nominal_template(name) != name.as_str() {
                bindings.self_instance =
                    Some((nominal_template(name).to_string(), receiver.clone()));
                owner_covered =
                    owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
            }
        }
        let own_binders: Vec<ParamRef> = declaration
            .param_decls
            .iter()
            .filter(|decl| matches!(decl, ParamDecl::Type { .. }))
            .map(ParamDecl::binder)
            .filter(|binder| !bindings.types.contains_key(binder))
            .collect();
        for (binder, ty) in own_binders.into_iter().zip(method_bindings) {
            bindings.types.insert(binder, ty.clone());
        }
        for (pattern, actual) in declaration.param_types.iter().zip(argument_types) {
            unify(pattern, actual, &mut bindings)
                .map_err(|e| self.error(Some(owner), format!("monomorphizing `{target}`: {e}")))?;
        }
        for decl in &declaration.param_decls {
            if let ParamDecl::Type { .. } = decl
                && !bindings.types.contains_key(&decl.binder())
            {
                bindings.types.insert(decl.binder(), Ty::StringLiteral);
            }
        }
        for ty in &declaration.param_types {
            if let Ty::Param { binder, .. } = ty
                && !bindings.types.contains_key(binder)
            {
                bindings.types.insert(binder.clone(), Ty::StringLiteral);
            }
        }
        let mut arguments = ordered_arguments(&declaration.param_decls, &bindings, &target)?;
        arguments.drain(..owner_covered);
        push_sugar_arguments(declaration, &bindings, &mut arguments);
        self.enqueue(&target, bindings, arguments).map(Some)
    }
}

/// `ty` with a one-bound binder renamed to the existential spelling of its
/// bound (`H: Hasher` as `Some[Hasher]`).
fn existential_spelling(ty: &Ty) -> Ty {
    match ty {
        Ty::Param {
            binder,
            bounds,
            callable_bound: None,
        } if bounds.len() == 1 => Ty::Param {
            binder: mojito_types::param_expr::ParamRef {
                name: format!("Some[{}]", bounds[0]).into(),
                ..binder.clone()
            },
            bounds: bounds.clone(),
            callable_bound: None,
        },
        other => other.clone(),
    }
}

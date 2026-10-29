//! Derived-instance enqueueing: hash leaves, nominal methods,
//! display, and Intable instances.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

impl Specializer<'_> {
    /// The constant a value-parameter member read (`Self.length`) at `read`
    /// resolves to, when `field` names a value parameter (not a declared
    /// field) of the receiver's template and the instance type carries its
    /// solution.
    pub(super) fn value_param_constant(
        &self,
        receiver: &Ty,
        field: &str,
        read: Option<&Ty>,
    ) -> Option<Const> {
        let TyKind::Struct(name, type_args) = peel_refs(receiver).kind() else {
            return None;
        };
        let struct_decl = self.structs.get(nominal_template(name))?;
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
            CtValue::UInt(v) => Some(Const::Value(CtValue::UInt(*v))),
            CtValue::Index(v) => Some(Const::Value(CtValue::Index(*v))),
            CtValue::Bool(v) => Some(Const::Bool(*v)),
            CtValue::Dtype(v) => Some(Const::Dtype(*v)),
            value @ (CtValue::Simd { .. } | CtValue::Struct { .. }) => {
                Some(Const::Value(value.clone()))
            }
            CtValue::Str(_) if read.is_some_and(mojito_types::types::is_nominal_string) => {
                Some(Const::Value(value.clone()))
            }
            _ => None,
        }
    }

    /// Enqueue the instances a lowered builtin `__hash__(hasher)` leaf calls:
    /// for a `Bool` or `DType` receiver, the hasher's `_update_with_simd`
    /// instantiated at the type the leaf hashes as (`hash_leaf_ty`), its
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
        if !matches!(hasher_ty.kind(), TyKind::Struct(..)) {
            return Ok(());
        }
        if matches!(receiver.kind(), TyKind::Bool | TyKind::Dtype) {
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
        if matches!(receiver.kind(), TyKind::StringLiteral(_)) {
            let string = Ty::new(TyKind::Struct(
                mojito_symbol::symbol::STDLIB_STRING_STRUCT.to_string(),
                Vec::new(),
            ));
            self.enqueue_nominal_method_instance(
                owner,
                &string,
                "__hash__",
                1,
                std::slice::from_ref(&hasher_ty),
            )?;
        }
        if let TyKind::Variant(alternatives) = receiver.kind() {
            for alternative in alternatives {
                if matches!(alternative.kind(), TyKind::Struct(..)) {
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

    /// The instance one nominal method reached by a lowered intrinsic
    /// names, enqueued (see [`Self::enqueue_nominal_method_instance`]):
    /// `argument_types`, when given, solve the method's own binders as a
    /// call's arguments would before any remaining type parameter takes
    /// the builtin string. `None` when the receiver declares no such method.
    pub(super) fn nominal_method_instance(
        &mut self,
        owner: &str,
        receiver: &Ty,
        method: &str,
        argc: usize,
        method_bindings: &[Ty],
        argument_types: &[Ty],
    ) -> Result<Option<String>, MonoError> {
        let TyKind::Struct(name, arguments) = receiver.kind() else {
            return Ok(None);
        };
        let target = mojito_symbol::symbol::resolve_method_symbol(
            &self.functions,
            nominal_template(name),
            method,
            None,
            argc,
        );
        if !self.functions.contains_key(target.as_str()) {
            return Ok(None);
        }
        let Some(declaration) = self.declarations.get(target.as_str()) else {
            return Ok(None);
        };
        let mut bindings = self.base_bindings();
        let mut owner_covered = 0;
        if let Some(struct_decl) = self.structs.get(nominal_template(name)) {
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
                bindings.types.insert(decl.binder(), Ty::string_literal());
            }
        }
        for ty in &declaration.param_types {
            if let TyKind::Param { binder, .. } = ty.kind()
                && !bindings.types.contains_key(binder)
            {
                bindings.types.insert(binder.clone(), Ty::string_literal());
            }
        }
        let mut arguments =
            self.own_arguments(&declaration.param_decls, &bindings, &target, owner_covered)?;
        push_sugar_arguments(declaration, &bindings, &mut arguments);
        self.enqueue(&target, bindings, arguments).map(Some)
    }

    /// The member the receiver's template records in its witness table for
    /// the requirement a call through a bound selects (`dispatch`), as
    /// upstream's specialization folds `#kgen.get_witness`. A struct
    /// recording none fails as upstream's materialization does.
    pub(super) fn witness(
        &self,
        owner: &str,
        receiver_name: &str,
        dispatch: &str,
    ) -> Result<String, MonoError> {
        let template = nominal_template(receiver_name);
        self.structs
            .get(template)
            .and_then(|declaration| declaration.witness(dispatch))
            .map(str::to_string)
            .ok_or_else(|| {
                self.error(
                    Some(owner),
                    format!(
                        "struct `{template}` records no witness for the requirement `{dispatch}`"
                    ),
                )
            })
    }

    /// Whether the declared struct `receiver` is an instance of answers the
    /// requirement `dispatch` with a body: `SIMD`'s, for a `TyKind::Simd`
    /// receiver, once `std.simd` declares the witness. Any other requirement
    /// is the builtin's to answer.
    pub(super) fn declares_witness(&self, receiver: &Ty, dispatch: &str) -> bool {
        mojito_types::types::nominal_parts(receiver)
            .and_then(|(name, _)| self.structs.get(name))
            .and_then(|declaration| declaration.witness(dispatch))
            .is_some_and(|target| self.functions.contains_key(target))
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
        // A string literal's repr is its `as_string_slice()`'s, which the
        // backends write through the nominal `String`'s member (R1660).
        if function
            .reg_types
            .get(&arg.0)
            .is_some_and(|ty| peel_refs(ty).is_string_literal())
        {
            let string = Ty::new(TyKind::Struct(
                mojito_symbol::symbol::STDLIB_STRING_STRUCT.to_string(),
                Vec::new(),
            ));
            return self.enqueue_format_ty_instance(owner, &string, "write_repr_to");
        }
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
        self.enqueue_format_ty_instance(owner, peel_refs(ty), method)
    }

    fn enqueue_format_ty_instance(
        &mut self,
        owner: &str,
        ty: &Ty,
        method: &str,
    ) -> Result<(), MonoError> {
        let TyKind::Struct(name, arguments) = ty.kind() else {
            return Ok(());
        };
        let exact = format!("{name}.{method}");
        if self.functions.contains_key(exact.as_str()) {
            if self.writes_protocol(&exact) {
                self.enqueue_nominal_method_instance(owner, ty, method, 1, &[])?;
            }
            return Ok(());
        }
        // The template's `write_to` is instantiated for the receiver.
        let target = mojito_symbol::symbol::resolve_method_symbol(
            &self.functions,
            nominal_template(name),
            method,
            None,
            1,
        );
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
        let Some(declaration) = self.declarations.get(target.as_str()) else {
            return Ok(());
        };
        let mut bindings = self.base_bindings();
        let mut owner_covered = 0;
        if let Some(struct_decl) = self.structs.get(nominal_template(name)) {
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
                bindings.types.insert(decl.binder(), Ty::string_literal());
            }
        }
        // The `Some[Writer]` sugar parameter is infer-only (absent from
        // `param_decls`); bind its spelling from the declared type.
        for ty in &declaration.param_types {
            if let TyKind::Param { binder, .. } = ty.kind()
                && !bindings.types.contains_key(binder)
            {
                bindings.types.insert(binder.clone(), Ty::string_literal());
            }
        }
        let arguments =
            self.own_arguments(&declaration.param_decls, &bindings, &target, owner_covered)?;
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
        let Some(ty) = function.reg_types.get(&arg.0) else {
            return Ok(());
        };
        let TyKind::Struct(name, _) = ty.kind() else {
            return Ok(());
        };
        let target = mojito_symbol::symbol::resolve_method_symbol(
            &self.functions,
            nominal_template(name),
            "__int__",
            None,
            0,
        );
        if !self.functions.contains_key(target.as_str()) {
            return Ok(());
        }
        let (bindings, arguments, _) =
            self.infer_receiver_call(owner, &target, ty, ReceiverCall::default())?;
        self.enqueue(&target, bindings, arguments)?;
        Ok(())
    }

    /// Whether a display method takes the `Writable` protocol's writer; any
    /// other `write_to` is an ordinary overload and never serves display.
    /// The one overload of `template.method` whose first parameter is a
    /// `Writer`.
    fn protocol_overload(&self, template: &str, method: &str) -> Option<String> {
        let family = format!("{template}.{method}{}", mojito_symbol::symbol::OV_SEP);
        let mut overloads = self.declarations.prefixed(&family).filter(|name| {
            self.declarations.get(name).is_some_and(|declaration| {
                declaration
                    .param_types
                    .first()
                    .is_some_and(mojito_types::types::is_writer_parameter)
            })
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
}

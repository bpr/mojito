//! Derived-instance enqueueing: hash leaves, nominal methods,
//! display, and Intable instances.

use super::*;

impl<'a> Specializer<'a> {
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
            _ => None,
        }
    }

    /// Enqueue the instances a lowered scalar `__hash__(hasher)` leaf calls:
    /// the hasher's `_update_with_simd`, and for a string-literal receiver
    /// the nominal String's `__hash__` bound to that hasher (the VM
    /// materializes the literal and dispatches the same way).
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
        self.enqueue_nominal_method_instance(owner, &hasher_ty, "_update_with_simd", 1, &[])?;
        if matches!(receiver, Ty::StringLiteral) {
            let string = Ty::Struct(crate::symbol::STDLIB_STRING_STRUCT.to_string(), Vec::new());
            self.enqueue_nominal_method_instance(
                owner,
                &string,
                "__hash__",
                1,
                &[("H", hasher_ty.clone())],
            )?;
        }
        Ok(())
    }

    /// Enqueue one nominal method instance reached by a lowered intrinsic
    /// rather than an explicit MIR call: the receiver binds the owner's
    /// parameters, `method_bindings` bind the method's own type parameters,
    /// and any remaining type parameter (a `Some[..]` sugar spelling)
    /// instantiates at the builtin string, as `enqueue_display_instance`.
    pub(super) fn enqueue_nominal_method_instance(
        &mut self,
        owner: &str,
        receiver: &Ty,
        method: &str,
        argc: usize,
        method_bindings: &[(&str, Ty)],
    ) -> Result<(), MonoError> {
        let Ty::Struct(name, arguments) = receiver else {
            return Ok(());
        };
        let target = crate::symbol::resolve_method_symbol(
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
                bindings.self_instance =
                    Some((nominal_template(name).to_string(), receiver.clone()));
                owner_covered =
                    owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
            }
        }
        for (parameter, ty) in method_bindings {
            bindings.types.insert((*parameter).to_string(), ty.clone());
        }
        for decl in &declaration.param_decls {
            if let ParamDecl::Type { name, .. } = decl
                && !bindings.types.contains_key(name.as_str())
            {
                bindings.types.insert(name.clone(), Ty::StringLiteral);
            }
        }
        for ty in &declaration.param_types {
            if let Ty::Param { name, .. } = ty
                && !bindings.types.contains_key(name.as_str())
            {
                bindings.types.insert(name.clone(), Ty::StringLiteral);
            }
        }
        let mut arguments = ordered_arguments(&declaration.param_decls, &bindings, &target)?;
        arguments.drain(..owner_covered);
        push_sugar_arguments(declaration, &bindings, &mut arguments);
        self.enqueue(&target, bindings, arguments)?;
        Ok(())
    }

    /// Enqueue the `write_to` instance a lowered `print` of a nominal struct
    /// calls — the VM's `format_value` dispatch. The receiver binds the
    /// owner's parameters; the writer parameter instantiates at the builtin
    /// string (the VM's `Value::Str` accumulator).
    pub(super) fn enqueue_display_instance(
        &mut self,
        owner: &str,
        function: &MirFunction,
        arg: Reg,
    ) -> Result<(), MonoError> {
        let Some(ty) = function.reg_types.get(&arg.0) else {
            return Ok(());
        };
        let ty = peel_refs(ty).clone();
        let Ty::Struct(name, arguments) = &ty else {
            return Ok(());
        };
        if crate::symbol::is_stdlib_string_struct(name) {
            return Ok(());
        }
        let target = crate::symbol::resolve_method_symbol(
            self.functions.iter().map(|(name, f)| CallableCandidate {
                name,
                n_params: f.n_params,
            }),
            nominal_template(name),
            "write_to",
            None,
            1,
        );
        if !self.functions.contains_key(target.as_str()) {
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
            if let ParamDecl::Type { name, .. } = decl
                && !bindings.types.contains_key(name.as_str())
            {
                bindings.types.insert(name.clone(), Ty::StringLiteral);
            }
        }
        // The `Some[Writer]` sugar parameter is infer-only (absent from
        // `param_decls`); bind its spelling from the declared type.
        for ty in &declaration.param_types {
            if let Ty::Param { name, .. } = ty
                && !bindings.types.contains_key(name.as_str())
            {
                bindings.types.insert(name.clone(), Ty::StringLiteral);
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
        let target = crate::symbol::resolve_method_symbol(
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
        let (bindings, arguments, _) = self.infer_receiver_call(owner, &target, ty, None)?;
        self.enqueue(&target, bindings, arguments)?;
        Ok(())
    }
}

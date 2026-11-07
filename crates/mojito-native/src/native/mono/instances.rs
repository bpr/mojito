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
        let (name, type_args) = peel_refs(receiver).kind().as_non_simd_struct()?;
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

    /// Enqueue the instances a lowered builtin `__hash__(hasher)` over a
    /// Variant receiver calls: the `__hash__` of every nominal alternative
    /// (the lowered tag switch dispatches whichever is active).
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
        if !hasher_ty.kind().is_non_simd_struct() {
            return Ok(());
        }
        if let TyKind::Variant(alternatives) = receiver.kind() {
            for alternative in alternatives {
                if alternative.kind().is_non_simd_struct() {
                    self.nominal_method_instance(
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

    /// The instance one nominal method reached by a lowered intrinsic
    /// rather than an explicit MIR call names, enqueued: the receiver binds
    /// the owner's parameters and `argument_types` solve the method's own
    /// binders as a call's arguments would. Every type parameter must be
    /// solved, as upstream never elaborates a call with an unsolved one.
    /// `None` when the receiver declares no such method.
    pub(super) fn nominal_method_instance(
        &mut self,
        owner: &str,
        receiver: &Ty,
        method: &str,
        argc: usize,
        argument_types: &[Ty],
    ) -> Result<Option<String>, MonoError> {
        let Some((name, arguments)) = struct_receiver(receiver) else {
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
            bind_ty_args(&struct_decl.param_decls, &arguments, &mut bindings).map_err(|e| {
                self.error(
                    Some(owner),
                    format!("monomorphizing receiver for `{target}`: {e}"),
                )
            })?;
            if nominal_template(name) != name {
                bindings.self_instance =
                    Some((nominal_template(name).to_string(), receiver.clone()));
                owner_covered =
                    owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
            }
        }
        for (pattern, actual) in declaration.param_types.iter().zip(argument_types) {
            unify(pattern, actual, &mut bindings)
                .map_err(|e| self.error(Some(owner), format!("monomorphizing `{target}`: {e}")))?;
        }
        let unsolved = declaration
            .param_decls
            .iter()
            .filter(|decl| matches!(decl, ParamDecl::Type { .. }))
            .map(ParamDecl::binder)
            .chain(
                declaration
                    .param_types
                    .iter()
                    .filter_map(|ty| match ty.kind() {
                        TyKind::Param { binder, .. } => Some(binder.clone()),
                        _ => None,
                    }),
            )
            .find(|binder| !bindings.types.contains_key(binder));
        if let Some(binder) = unsolved {
            return Err(self.error(
                Some(owner),
                format!(
                    "monomorphizing `{target}`: its arguments leave the type parameter `{}` unsolved",
                    binder.name
                ),
            ));
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

    /// The `__call__` the receiver's template records for its `def(...)`
    /// conformance, which a call through a `def(...)` bound runs on a struct
    /// instance. A struct declaring none fails as [`Self::witness`] does.
    pub(super) fn callable_witness(
        &self,
        owner: &str,
        receiver_name: &str,
    ) -> Result<String, MonoError> {
        let template = nominal_template(receiver_name);
        self.structs
            .get(template)
            .and_then(|declaration| declaration.callable_witness())
            .map(str::to_string)
            .ok_or_else(|| {
                self.error(
                    Some(owner),
                    format!("struct `{template}` records no witness for a `def(...)` conformance"),
                )
            })
    }

    /// Whether the declared struct `receiver` is an instance of answers the
    /// requirement `dispatch` with a body: `SIMD`'s, for a `SIMD`
    /// receiver, once `std.simd` declares the witness. Any other requirement
    /// is the builtin's to answer.
    pub(super) fn declares_witness(&self, receiver: &Ty, dispatch: &str) -> bool {
        mojito_types::types::nominal_parts(receiver)
            .and_then(|(name, _)| self.structs.get(name))
            .and_then(|declaration| declaration.witness(dispatch))
            .is_some_and(|target| self.functions.contains_key(target))
    }

    /// A writer's host-formatted write (`writer.$write_formatted(x)`)
    /// reaches the writer's `write_string`: enqueue its instance for the
    /// lowered expansion.
    pub(super) fn enqueue_formatted_write(
        &mut self,
        owner: &str,
        function: &MirFunction,
        writer: Reg,
    ) -> Result<(), MonoError> {
        let Some(writer_ty) = function
            .reg_types
            .get(&writer.0)
            .map(|ty| peel_refs(ty).clone())
        else {
            return Ok(());
        };
        let Some((writer_name, _)) = mojito_types::types::nominal_parts(&writer_ty) else {
            return Ok(());
        };
        let write_string = mojito_symbol::symbol::resolve_callable_symbol(
            &self.functions,
            &format!("{}.write_string", nominal_template(writer_name)),
            1,
        );
        if !self.functions.contains_key(write_string.as_str()) {
            return Ok(());
        }
        let (bindings, arguments, _) =
            self.infer_receiver_call(owner, &write_string, &writer_ty, ReceiverCall::default())?;
        self.enqueue(&write_string, bindings, arguments)?;
        Ok(())
    }

    /// Whether a display method takes the `Writable` protocol's writer; any
    /// other `write_to` is an ordinary overload and never serves display.
    pub(super) fn writes_protocol(&self, target: &str) -> bool {
        self.declarations.get(target).is_none_or(|declaration| {
            declaration
                .param_types
                .first()
                .is_some_and(mojito_types::types::is_writer_parameter)
        })
    }
}

/// The struct and arguments whose member table a receiver of type `ty`
/// reads: a non-`SIMD` struct's own.
fn struct_receiver(ty: &Ty) -> Option<(&str, std::borrow::Cow<'_, [mojito_types::types::TyArg]>)> {
    mojito_types::types::nominal_parts(ty).filter(|_| ty.kind().is_non_simd_struct())
}

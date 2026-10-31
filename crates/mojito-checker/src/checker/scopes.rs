//! Lexical scope and binding management for the checker: value/owner scopes,
//! declaration and mutability, callable-origin registration, and nested-def
//! capture-access checks. Extracted from `checker.rs`; see `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::FuncTy;
use mojito_types::types::TyKind;

impl Checker {
    /// Allocate a stable identity for a source binding's storage (or for a
    /// materialized borrow-source temporary, which has no source name).
    pub(super) fn fresh_owner(&self) -> Result<mojito_types::origin::OwnerId, TypeError> {
        Ok(mojito_types::origin::OwnerId(self.reserve_owners(1)?))
    }

    /// The identity the next `fresh_owner` returns.
    pub(super) const fn peek_owner(&self) -> mojito_types::origin::OwnerId {
        mojito_types::origin::OwnerId(self.owner_cursor(1))
    }

    /// Allocate `count` consecutive identities and return the first.
    ///
    /// A body inferred again under a record allocates from the range it had
    /// (`body_carry`); a block that would cross that range's ceiling moves
    /// to the fresh region above every recorded range, once, and notes the
    /// split (`owner_range_split`).
    pub(super) fn reserve_owners(&self, count: u32) -> Result<u32, TypeError> {
        let start = self.owner_cursor(count);
        if start != self.next_owner.get() {
            self.owner_ceiling.set(None);
            self.owner_range_split.set(true);
        }
        let end = start.checked_add(count).ok_or_else(|| {
            TypeError::InvariantViolation("checker exhausted binding identities".to_string())
        })?;
        self.next_owner.set(end);
        if self.owner_ceiling.get().is_none() {
            self.fresh_owner_cursor
                .set(self.fresh_owner_cursor.get().max(end));
        }
        Ok(start)
    }

    /// Where a block of `count` identities starts: the cursor, or the fresh
    /// region when the block would cross the current ceiling.
    const fn owner_cursor(&self, count: u32) -> u32 {
        let next = self.next_owner.get();
        match self.owner_ceiling.get() {
            Some(ceiling) if next.saturating_add(count) > ceiling => self.fresh_owner_cursor.get(),
            _ => next,
        }
    }

    /// Bind `name` in the innermost scope. Repeated function declarations form an
    /// overload set when their call shapes differ; other same-scope repeats remain
    /// redeclarations.
    pub(super) fn declare(&mut self, name: &str, ty: Ty) -> Result<(), TypeError> {
        self.declare_with_mutability(name, ty, true)
    }

    pub(super) fn declare_immutable(&mut self, name: &str, ty: Ty) -> Result<(), TypeError> {
        self.declare_with_mutability(name, ty, false)
    }

    /// Bind a value parameter in its declaration's body: an immutable slot
    /// that nested functions read without an explicit capture.
    pub(super) fn declare_value_parameter(&mut self, name: &str, ty: Ty) -> Result<(), TypeError> {
        self.declare_immutable(name, ty)?;
        if let Some(owner) = self.lookup_owner(name) {
            self.value_parameter_owners.insert(owner);
        }
        Ok(())
    }

    /// Bind a loop or comprehension target `name` as `plan` declares it. A
    /// target not declared `var` binds the element by reference.
    pub(super) fn declare_loop_binder(
        &mut self,
        name: &str,
        plan: &mojito_checked::checked::CheckedIterationBinding,
    ) -> Result<(), TypeError> {
        self.declare_with_mutability(name, plan.binding_ty.clone(), plan.mutable)?;
        if plan.mode != mojito_ast::ast::BindingMode::Var
            && let Some(owner) = self.lookup_owner(name)
        {
            self.reference_binders.insert(owner);
        }
        Ok(())
    }

    /// Bind every value parameter among `decls`, a variadic one as its pack.
    pub(super) fn declare_value_parameters(
        &mut self,
        decls: &[ParamDecl],
    ) -> Result<(), TypeError> {
        decls.iter().try_for_each(|decl| match decl {
            ParamDecl::Value {
                name, ty, variadic, ..
            } => self.declare_value_parameter(
                name.trim_start_matches('*'),
                match decl.value_element_ty() {
                    Some(element) if *variadic => TyKind::VariadicPack(element.clone()).into(),
                    _ => (*ty).clone(),
                },
            ),
            ParamDecl::Type { .. } => Ok(()),
        })
    }

    /// Resolve a signature inside its own value scope, as upstream's
    /// `Signatures.cpp` emits each type with the earlier arguments in scope:
    /// the receiver (`receiver`, a method's `Self`) is declared first, and
    /// [`Self::param_tys`] declares each parameter as its annotation
    /// resolves, so a later annotation or the result reads it (`y:
    /// type_of(x)`, `-> type_of(self.v)`).
    pub(super) fn in_signature_scope<R>(
        &mut self,
        receiver: Option<&Ty>,
        resolve: impl FnOnce(&mut Self) -> Result<R, TypeError>,
    ) -> Result<R, TypeError> {
        self.push_scope();
        self.signature_scopes += 1;
        let resolved = receiver
            .map_or(Ok(()), |ty| {
                self.declare_signature_parameter("self", ty.clone())
            })
            .and_then(|()| resolve(self));
        self.signature_scopes -= 1;
        self.pop_scope();
        resolved
    }

    /// Whether checking is outside every body: in a module statement, a
    /// field type, or a signature, whose scopes no body owns.
    pub(super) const fn outside_bodies(&self) -> bool {
        self.scopes.len() - self.signature_scopes < 2
    }

    /// Bind a signature's parameter `name` at `ty` in the innermost scope:
    /// a typed name with no storage, which only the signature's later
    /// annotations read.
    pub(super) fn declare_signature_parameter(
        &mut self,
        name: &str,
        ty: Ty,
    ) -> Result<(), TypeError> {
        self.scopes
            .last_mut()
            .ok_or_else(|| {
                TypeError::InvariantViolation("checker scope stack is empty".to_string())
            })?
            .insert(name.to_string(), ty);
        self.mutable_scopes
            .last_mut()
            .ok_or_else(|| {
                TypeError::InvariantViolation("checker mutability scope stack is empty".to_string())
            })?
            .insert(name.to_string(), false);
        Ok(())
    }

    /// Bind `name` in the innermost scope as another name of the immutable
    /// binding `target` resolves to: a read of either is a read of the one
    /// binding, with its type and its owner.
    pub(super) fn declare_alias(&mut self, name: &str, target: &str) -> Result<(), TypeError> {
        let (Some(ty), Some(owner)) = (self.lookup(target).cloned(), self.lookup_owner(target))
        else {
            return Err(TypeError::InvariantViolation(format!(
                "alias '{name}' names '{target}', which is not in scope"
            )));
        };
        let scope = self.scopes.last_mut().ok_or_else(|| {
            TypeError::InvariantViolation("checker scope stack is empty".to_string())
        })?;
        if scope.contains_key(name) {
            return Err(TypeError::Redeclaration(name.to_string()));
        }
        scope.insert(name.to_string(), ty);
        self.mutable_scopes
            .last_mut()
            .ok_or_else(|| {
                TypeError::InvariantViolation("checker mutability scope stack is empty".to_string())
            })?
            .insert(name.to_string(), false);
        self.owner_scopes
            .last_mut()
            .ok_or_else(|| {
                TypeError::InvariantViolation("checker owner scope stack is empty".to_string())
            })?
            .insert(name.to_string(), owner);
        Ok(())
    }

    pub(super) fn declare_with_mutability(
        &mut self,
        name: &str,
        ty: Ty,
        mutable: bool,
    ) -> Result<(), TypeError> {
        let nested_scope = self.scopes.len() > 1;
        let predeclared = !nested_scope && self.predeclared_constants.remove(name);
        let scope = self.scopes.last_mut().ok_or_else(|| {
            TypeError::InvariantViolation("checker scope stack is empty".to_string())
        })?;
        if let Some(existing) = scope.get_mut(name)
            && !predeclared
        {
            if let Some(mut candidates) = overload_candidates(existing, &ty) {
                if nested_scope {
                    return Err(TypeError::Unsupported("overloaded nested def".to_string()));
                }
                if candidates
                    .iter()
                    .any(|candidate| same_callable_signature(candidate, &ty))
                {
                    return Err(TypeError::Redeclaration(name.to_string()));
                }
                candidates.push(ty);
                *existing = TyKind::Overload(candidates).into();
                return Ok(());
            }
            return Err(TypeError::Redeclaration(name.to_string()));
        }
        scope.insert(name.to_string(), ty);
        self.mutable_scopes
            .last_mut()
            .ok_or_else(|| {
                TypeError::InvariantViolation("checker mutability scope stack is empty".to_string())
            })?
            .insert(name.to_string(), mutable);
        let owner = self.fresh_owner()?;
        self.owner_scopes
            .last_mut()
            .ok_or_else(|| {
                TypeError::InvariantViolation("checker owner scope stack is empty".to_string())
            })?
            .insert(name.to_string(), owner);
        Ok(())
    }

    /// Set whether the binding `name` in the innermost scope that holds it
    /// may be written: a `case` binding is immutable whatever its subject.
    pub(super) fn set_binding_mutable(&mut self, name: &str, mutable: bool) {
        if let Some(scope) = self
            .mutable_scopes
            .iter_mut()
            .rev()
            .find(|scope| scope.contains_key(name))
        {
            scope.insert(name.to_string(), mutable);
        }
    }

    /// Retype the innermost binding of `name` as `ty`.
    pub(super) fn set_binding_type(&mut self, name: &str, ty: Ty) {
        if let Some(scope) = self
            .scopes
            .iter_mut()
            .rev()
            .find(|scope| scope.contains_key(name))
        {
            scope.insert(name.to_string(), ty);
        }
    }

    pub(super) fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
        self.mutable_scopes.push(HashMap::new());
        self.owner_scopes.push(HashMap::new());
        self.aggregate_origin_scopes.push(HashMap::new());
        self.aggregate_field_origin_scopes.push(HashMap::new());
        self.storage_origin_demand_scopes.push(HashMap::new());
        self.immutable_origin_binder_scopes.push(HashMap::new());
        self.reference_parameter_scopes.push(HashMap::new());
        self.callable_origin_scopes.push(HashMap::new());
        self.local_type_aliases.push(HashMap::new());
        self.local_type_alias_values.push(HashMap::new());
        self.local_comptime_values.push(HashMap::new());
        self.local_comptime_parameters.push(HashMap::new());
        self.comptime_dtypes.push(HashMap::new());
        self.local_comptime_displays.push(HashMap::new());
        self.compile_time_bindings.push(HashSet::new());
    }

    pub(super) fn pop_scope(&mut self) {
        if let Some(owners) = self.owner_scopes.last() {
            let ids: HashSet<_> = owners.values().copied().collect();
            self.uninitialized
                .borrow_mut()
                .retain(|owner| !ids.contains(owner));
        }
        self.scopes.pop();
        self.mutable_scopes.pop();
        self.owner_scopes.pop();
        self.aggregate_origin_scopes.pop();
        self.aggregate_field_origin_scopes.pop();
        self.storage_origin_demand_scopes.pop();
        self.immutable_origin_binder_scopes.pop();
        self.reference_parameter_scopes.pop();
        self.callable_origin_scopes.pop();
        self.local_type_aliases.pop();
        self.local_type_alias_values.pop();
        self.local_comptime_values.pop();
        self.local_comptime_parameters.pop();
        self.comptime_dtypes.pop();
        self.local_comptime_displays.pop();
        self.compile_time_bindings.pop();
    }

    /// Whether the binding `name` resolves to is a compile-time binding of an
    /// enclosing validated body (a `comptime for` variable, a local
    /// `comptime` binding, or a value parameter) — a compile-time constant,
    /// not runtime storage.
    pub(super) fn is_compile_time_binding(&self, name: &str) -> bool {
        match self
            .scopes
            .iter()
            .rposition(|scope| scope.contains_key(name))
        {
            Some(scope) => self
                .compile_time_bindings
                .get(scope)
                .is_some_and(|bindings| bindings.contains(name)),
            None => self
                .installed_binding(name)
                .is_some_and(|(_, binding)| binding.compile_time),
        }
    }

    /// Open a compile-time position, closed when the guard drops.
    pub(super) fn comptime_position(&self) -> ComptimePosition {
        self.comptime_positions
            .set(self.comptime_positions.get() + 1);
        ComptimePosition(std::rc::Rc::clone(&self.comptime_positions))
    }

    /// Open a position whose compile-time expressions are lifted when they
    /// compile to no parameter expression ([`Self::lifted_application`]),
    /// closed when the guard drops.
    pub(super) fn lifting_position(&self) -> ComptimePosition {
        self.lifting_positions.set(self.lifting_positions.get() + 1);
        ComptimePosition(std::rc::Rc::clone(&self.lifting_positions))
    }

    /// Whether a compile-time collection read where the check stands would
    /// cross to runtime: the executable check, outside every compile-time
    /// position. Source validation leaves the crossing to the elaborator.
    pub(super) fn crosses_to_runtime(&self) -> bool {
        self.comptime_positions.get() == 0
    }

    pub(super) fn is_binding_mutable(&self, name: &str) -> bool {
        self.mutable_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
            .or_else(|| {
                self.installed_binding(name)
                    .map(|(_, binding)| binding.mutable)
            })
            .unwrap_or(true)
    }

    /// The type to declare for an **inferred** binding — `var x = e` (no
    /// annotation) or a var-less `x = e`. A numeric literal materializes to its
    /// default kind (`default_literal`); a value that cannot live in a named
    /// binding is rejected: a closure (`ClosureEscape`, matching `return`/reassign)
    /// or another value outside the source language's first-class surface. A
    /// `def(...) thin` value captures nothing, so it binds like any other
    /// value, and a closure's storage is a value of its own struct.
    #[allow(
        clippy::unused_self,
        reason = "TODO: make an associated function or use the receiver"
    )]
    pub(super) fn inferred_binding_ty(&self, value_ty: &Ty, _name: &str) -> Result<Ty, TypeError> {
        match value_ty.kind() {
            TyKind::Func(func)
                if let FuncTy {
                    environment:
                        mojito_types::origin::CallableEnvironment::Thin
                        | mojito_types::origin::CallableEnvironment::Storage(_),
                    ..
                } = &**func =>
            {
                Ok(value_ty.clone())
            }
            TyKind::Func(_) | TyKind::GenericFunc(_) | TyKind::Overload(_) => {
                Err(TypeError::ClosureEscape)
            }
            _ => Ok(default_literal(value_ty)),
        }
    }

    /// The type to declare for an unannotated `comptime` binding: its
    /// initializer's type, as upstream's alias has. A float literal stays a
    /// `FloatLiteral` that materializes at each read; any other value binds
    /// as an inferred binding does.
    pub(super) fn comptime_binding_ty(&self, value_ty: &Ty, name: &str) -> Result<Ty, TypeError> {
        if matches!(value_ty.kind(), TyKind::FloatLiteral) {
            return Ok(value_ty.clone());
        }
        self.inferred_binding_ty(value_ty, name)
    }

    /// Look up `name`, walking outward through the scope chain (lexical lookup).
    /// A package module binding resolves below the module scope.
    pub(super) fn lookup(&self, name: &str) -> Option<&Ty> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .or_else(|| self.installed_binding(name).map(|(_, binding)| &binding.ty))
    }

    /// The package module binding `name` resolves to below the module
    /// scope, with its owner, resolving its declaration's record on first
    /// lookup.
    pub(super) fn installed_binding(
        &self,
        name: &str,
    ) -> Option<(mojito_types::origin::OwnerId, &interface::ModuleBinding)> {
        let (installed, base) = self.installed.as_ref()?;
        let (ordinal, binding) = installed.binding(name)?;
        Some((mojito_types::origin::OwnerId(base + ordinal), binding))
    }

    /// Whether `owner` is a module `comptime` binding a compile-time
    /// expression may read.
    pub(super) fn is_comptime_binding_owner(&self, owner: mojito_types::origin::OwnerId) -> bool {
        self.comptime_binding_owners.contains(&owner)
            || self
                .installed_owner(owner)
                .is_some_and(|(_, binding)| binding.comptime)
    }

    pub(super) fn register_callable_origins(
        &mut self,
        name: &str,
        signature: CallableOriginSignature,
    ) {
        self.callable_origin_scopes
            .last_mut()
            .expect("checker origin-callable scope stack is not empty")
            .entry(name.to_string())
            .or_default()
            .push(signature);
    }

    pub(super) fn lookup_callable_origins(
        &self,
        name: &str,
    ) -> Option<Vec<CallableOriginSignature>> {
        self.callable_origin_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
            .or_else(|| {
                (!self.scopes.iter().any(|scope| scope.contains_key(name)))
                    .then(|| self.installed_binding(name)?.1.callable_origins.clone())
                    .flatten()
            })
    }

    /// The scope binding `name`; a package module binding's is the module
    /// scope.
    pub(super) fn binding_scope(&self, name: &str) -> Option<usize> {
        self.scopes
            .iter()
            .rposition(|scope| scope.contains_key(name))
            .or_else(|| self.installed_binding(name).map(|_| 0))
    }

    /// Reject a nested function naming itself from inside its own body — a
    /// call, a value read, or either from a function nested deeper still —
    /// as upstream does: a recursive helper belongs at file scope.
    pub(super) fn reject_nested_self_reference(&self, name: &str) -> Result<(), TypeError> {
        let Some(scope) = self.binding_scope(name) else {
            return Ok(());
        };
        let recursive =
            self.capture_contexts.borrow().iter().any(|policy| {
                !policy.lambda && policy.function_name == name && scope < policy.base
            });
        if recursive {
            return Err(TypeError::Unsupported(format!(
                "recursive reference to nested function '{name}'; define '{name}' at file scope"
            )));
        }
        Ok(())
    }

    pub(super) fn check_capture_access(&self, name: &str, writing: bool) -> Result<(), TypeError> {
        self.reject_nested_self_reference(name)?;
        let contexts = self.capture_contexts.borrow();
        if contexts.is_empty() {
            return Ok(());
        }
        let Some(scope) = self.binding_scope(name) else {
            return Ok(());
        };
        // Module globals are not captures, and neither is a compile-time
        // binding (a literal once elaborated), an enclosing declaration's
        // value parameter, which a nested function reads as its own
        // parameter, or a local alias of an application, which each read
        // materializes. For a value crossing more than one nested-function
        // boundary, every intervening environment must forward it: an inner
        // `{value}` cannot tunnel through a middle `{}`.
        if scope == 0
            || self.is_compile_time_binding(name)
            || self.is_value_parameter(name)
            || self.local_comptime_application(name).is_some()
        {
            return Ok(());
        }
        for policy in contexts
            .iter()
            .filter(|policy| scope < policy.base && name != policy.function_name)
        {
            let kind = self.policy_capture_kind(policy, name);
            if let Some(kind) = kind {
                self.check_capture_capability(name, kind)?;
                let binding = self.lookup_owner(name).ok_or_else(|| {
                    TypeError::InvariantViolation(format!(
                        "capture '{name}' lost its checked binding"
                    ))
                })?;
                let ty = self.lookup(name).cloned().ok_or_else(|| {
                    TypeError::InvariantViolation(format!(
                        "capture '{name}' lost its checked storage type"
                    ))
                })?;
                let checked = self.checked_capture(name, binding, ty, kind);
                let mut record = self.record_at(policy.declaration.clone());
                let captures = record.captures.get_or_insert_default();
                if !captures.iter().any(|capture| capture.binding == binding) {
                    captures.push(checked);
                }
            }
            match (kind, writing) {
                (Some(mojito_ast::ast::CaptureKind::Mut), _) if self.is_binding_mutable(name) => {}
                (Some(mojito_ast::ast::CaptureKind::Ref), false) => {}
                (Some(mojito_ast::ast::CaptureKind::Ref), true)
                    if self.is_binding_mutable(name) => {}
                // A transferred capture is owned by the closure and retains
                // mutable state across calls. A copied capture is an immutable
                // snapshot in current Mojo.
                (Some(mojito_ast::ast::CaptureKind::Move), _) => {}
                (
                    Some(mojito_ast::ast::CaptureKind::Copy | mojito_ast::ast::CaptureKind::Imm),
                    false,
                ) => {}
                (Some(_), true) | (Some(mojito_ast::ast::CaptureKind::Mut), false) => {
                    return Err(TypeError::ImmutableBinding(name.to_string()));
                }
                (None, _) if policy.lambda => {
                    return Err(TypeError::Unsupported(format!(
                        "could not infer capture convention for '{name}': the lambda's \
                         explicit '{{}}' capture list captures nothing"
                    )));
                }
                (None, _) => {
                    return Err(TypeError::Unsupported(format!(
                        "Could not infer capture convention of the captured value {name}"
                    )));
                }
            }
        }
        Ok(())
    }

    /// How the innermost enclosing nested function captures `name`, when
    /// `name` crosses its boundary: `None` for its own local, a global, a
    /// compile-time binding, or a value parameter, none of which is a
    /// capture.
    pub(super) fn capture_kind(&self, name: &str) -> Option<mojito_ast::ast::CaptureKind> {
        let scope = self.binding_scope(name)?;
        if scope == 0
            || self.is_compile_time_binding(name)
            || self.is_value_parameter(name)
            || self.local_comptime_application(name).is_some()
        {
            return None;
        }
        let contexts = self.capture_contexts.borrow();
        let policy = contexts.last()?;
        (scope < policy.base && name != policy.function_name)
            .then(|| self.policy_capture_kind(policy, name))
            .flatten()
    }

    /// Record a name-based call as a capture when an enclosing policy permits
    /// it, without changing the historical rule that synthesized sibling calls
    /// may be reconstructed when no capture policy names them.
    pub(super) fn record_permitted_call_capture(&self, name: &str) {
        let contexts = self.capture_contexts.borrow();
        let Some(scope) = self.binding_scope(name) else {
            return;
        };
        if scope == 0 || self.is_value_parameter(name) {
            return;
        }
        let Some(binding) = self.lookup_owner(name) else {
            return;
        };
        let Some(ty) = self.lookup(name).cloned() else {
            return;
        };
        for policy in contexts
            .iter()
            .filter(|policy| scope < policy.base && name != policy.function_name)
        {
            let Some(kind) = policy.entries.get(name).copied().or(policy.default) else {
                continue;
            };
            let checked = self.checked_capture(name, binding, ty.clone(), kind);
            let mut record = self.record_at(policy.declaration.clone());
            let captures = record.captures.get_or_insert_default();
            if !captures.iter().any(|capture| capture.binding == binding) {
                captures.push(checked);
            }
        }
    }

    pub(super) fn check_capture_capability(
        &self,
        name: &str,
        kind: mojito_ast::ast::CaptureKind,
    ) -> Result<(), TypeError> {
        let Some(ty) = self.lookup(name) else {
            return Ok(());
        };
        let missing = match kind {
            // `{var value}` performs an implicit copy at the nested-function
            // declaration. A merely explicitly Copyable value is therefore not
            // enough; current Mojo requires the stronger marker here.
            mojito_ast::ast::CaptureKind::Copy if !self.is_implicitly_copyable(ty) => {
                Some("ImplicitlyCopyable")
            }
            mojito_ast::ast::CaptureKind::Move if !self.is_movable(ty) => Some("Movable"),
            _ => None,
        };
        if let Some(trait_name) = missing {
            return Err(TypeError::TraitNotSatisfied {
                param: name.to_string(),
                ty: ty.to_string(),
                trait_name: trait_name.to_string(),
                reason: Some(format!(
                    "capture convention for '{name}' requires {trait_name}"
                )),
            });
        }
        Ok(())
    }

    pub(super) fn lookup_owner(&self, name: &str) -> Option<mojito_types::origin::OwnerId> {
        self.owner_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
            .or_else(|| self.installed_binding(name).map(|(owner, _)| owner))
    }

    /// The source name of an owner in scope (the inverse of
    /// [`Self::lookup_owner`]), for diagnostics that spell an origin as
    /// upstream's `origin_of(name)`.
    pub(super) fn owner_name(&self, owner: mojito_types::origin::OwnerId) -> Option<&str> {
        self.owner_scopes
            .iter()
            .rev()
            .find_map(|scope| {
                scope
                    .iter()
                    .find_map(|(name, candidate)| (*candidate == owner).then_some(name.as_str()))
            })
            .or_else(|| self.installed_owner(owner).map(|(name, _)| name))
    }

    pub(super) fn record_statement_binding(&self, statement: &Stmt, name: &str) {
        if let Some(owner) = self.lookup_owner(name) {
            self.record_at(statement.source_span()).statement_binding = Some(owner);
        }
    }

    pub(super) fn owner_is_mutable(&self, owner: mojito_types::origin::OwnerId) -> bool {
        self.owner_scopes
            .iter()
            .zip(&self.mutable_scopes)
            .rev()
            .find_map(|(owners, mutability)| {
                owners
                    .iter()
                    .find(|(_, id)| **id == owner)
                    .and_then(|(name, _)| mutability.get(name).copied())
            })
            .or_else(|| {
                self.installed_owner(owner)
                    .map(|(_, binding)| binding.mutable)
            })
            .unwrap_or(false)
    }

    /// The outermost scope binding `owner`; a package module binding's is
    /// the module scope.
    pub(super) fn owner_scope(&self, owner: mojito_types::origin::OwnerId) -> Option<usize> {
        self.owner_scopes
            .iter()
            .position(|owners| owners.values().any(|candidate| *candidate == owner))
            .or_else(|| self.installed_owner(owner).map(|_| 0))
    }

    /// Whether `name` binds a value parameter of a declaration in scope:
    /// a compile-time value a nested function reads as its own parameter,
    /// never as a capture.
    pub(super) fn is_value_parameter(&self, name: &str) -> bool {
        self.lookup_owner(name)
            .is_some_and(|owner| self.value_parameter_owners.contains(&owner))
    }

    /// The package module binding owning `owner`, by name.
    fn installed_owner(
        &self,
        owner: mojito_types::origin::OwnerId,
    ) -> Option<(&str, &interface::ModuleBinding)> {
        let (installed, base) = self.installed.as_ref()?;
        let name = installed.binding_name(owner.0.checked_sub(*base)?)?;
        self.installed_binding(name)
            .filter(|(found, _)| *found == owner)
            .map(|(_, binding)| (name, binding))
    }

    /// How `policy` captures `name`: its list entry, the list's default, or
    /// an `@__parameter` environment's implicit capture by the binding's
    /// mutability.
    fn policy_capture_kind(
        &self,
        policy: &CapturePolicy,
        name: &str,
    ) -> Option<mojito_ast::ast::CaptureKind> {
        policy
            .entries
            .get(name)
            .copied()
            .or(policy.default)
            .or_else(|| {
                policy.implicit_by_binding.then(|| {
                    if self.is_binding_mutable(name) {
                        mojito_ast::ast::CaptureKind::Mut
                    } else {
                        mojito_ast::ast::CaptureKind::Imm
                    }
                })
            })
    }
}

/// An open compile-time position ([`Checker::comptime_position`]) or
/// lifting position ([`Checker::lifting_position`]).
pub(super) struct ComptimePosition(std::rc::Rc<std::cell::Cell<usize>>);

impl Drop for ComptimePosition {
    fn drop(&mut self) {
        self.0.set(self.0.get().saturating_sub(1));
    }
}

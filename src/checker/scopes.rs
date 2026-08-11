//! Lexical scope and binding management for the checker: value/owner scopes,
//! declaration and mutability, callable-origin registration, and nested-def
//! capture-access checks. Extracted from `checker.rs`; see `docs/symbol-map.md`.

use super::*;

impl Checker {
    /// Allocate a stable identity for a source binding's storage.
    pub(super) fn fresh_owner(&mut self) -> Result<crate::origin::OwnerId, TypeError> {
        let owner = crate::origin::OwnerId(self.next_owner);
        self.next_owner = self.next_owner.checked_add(1).ok_or_else(|| {
            TypeError::InvariantViolation("checker exhausted binding identities".to_string())
        })?;
        Ok(owner)
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

    pub(super) fn declare_with_mutability(
        &mut self,
        name: &str,
        ty: Ty,
        mutable: bool,
    ) -> Result<(), TypeError> {
        let nested_scope = self.scopes.len() > 1;
        let scope = self.scopes.last_mut().ok_or_else(|| {
            TypeError::InvariantViolation("checker scope stack is empty".to_string())
        })?;
        if let Some(existing) = scope.get_mut(name) {
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
                *existing = Ty::Overload(candidates);
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

    pub(super) fn declare_function_implicit(
        &mut self,
        name: &str,
        ty: Ty,
    ) -> Result<(), TypeError> {
        let scope_index = self
            .function_bases
            .last()
            .copied()
            .unwrap_or(self.scopes.len().saturating_sub(1));
        if self.scopes[scope_index].contains_key(name) {
            return Err(TypeError::Redeclaration(name.to_string()));
        }
        self.scopes[scope_index].insert(name.to_string(), ty);
        self.mutable_scopes[scope_index].insert(name.to_string(), true);
        let owner = self.fresh_owner()?;
        self.owner_scopes[scope_index].insert(name.to_string(), owner);
        Ok(())
    }

    pub(super) fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
        self.mutable_scopes.push(HashMap::new());
        self.owner_scopes.push(HashMap::new());
        self.aggregate_origin_scopes.push(HashMap::new());
        self.aggregate_field_origin_scopes.push(HashMap::new());
        self.reference_parameter_scopes.push(HashMap::new());
        self.callable_origin_scopes.push(HashMap::new());
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
        self.reference_parameter_scopes.pop();
        self.callable_origin_scopes.pop();
    }

    pub(super) fn is_binding_mutable(&self, name: &str) -> bool {
        self.mutable_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
            .unwrap_or(true)
    }

    /// The type to declare for an **inferred** binding — `var x = e` (no
    /// annotation) or a var-less `x = e`. A numeric literal materializes to its
    /// default kind (`default_literal`); a value that cannot live in a named
    /// binding is rejected: a closure (`ClosureEscape`, matching `return`/reassign)
    /// or another value outside the source language's first-class surface.
    pub(super) fn inferred_binding_ty(&self, value_ty: &Ty, _name: &str) -> Result<Ty, TypeError> {
        match value_ty {
            Ty::Func { .. } | Ty::GenericFunc { .. } | Ty::Overload(_) => {
                Err(TypeError::ClosureEscape)
            }
            other => Ok(default_literal(other)),
        }
    }

    /// Look up `name`, walking outward through the scope chain (lexical lookup).
    pub(super) fn lookup(&self, name: &str) -> Option<&Ty> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name))
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
    }

    pub(super) fn binding_scope(&self, name: &str) -> Option<usize> {
        self.scopes
            .iter()
            .rposition(|scope| scope.contains_key(name))
    }

    pub(super) fn check_capture_access(&self, name: &str, writing: bool) -> Result<(), TypeError> {
        let contexts = self.capture_contexts.borrow();
        if contexts.is_empty() {
            return Ok(());
        }
        let Some(scope) = self.binding_scope(name) else {
            return Ok(());
        };
        // Module globals are not captures. For a value crossing more than one
        // nested-function boundary, every intervening environment must forward
        // it: an inner `{value}` cannot tunnel through a middle `{}`.
        if scope == 0 {
            return Ok(());
        }
        for policy in contexts
            .iter()
            .filter(|policy| scope < policy.base && name != policy.function_name)
        {
            let kind = policy.entries.get(name).copied().or(policy.default);
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
                let mut declarations = self.declaration_captures.borrow_mut();
                let captures = declarations.entry(policy.declaration.clone()).or_default();
                if !captures.iter().any(|capture| capture.binding == binding) {
                    captures.push(checked);
                }
            }
            match (kind, writing) {
                (Some(crate::ast::CaptureKind::Mut), _) if self.is_binding_mutable(name) => {}
                (Some(crate::ast::CaptureKind::Ref), false) => {}
                (Some(crate::ast::CaptureKind::Ref), true) if self.is_binding_mutable(name) => {}
                // A transferred capture is owned by the closure and retains
                // mutable state across calls. A copied capture is an immutable
                // snapshot in current Mojo.
                (Some(crate::ast::CaptureKind::Move), _) => {}
                (Some(crate::ast::CaptureKind::Copy | crate::ast::CaptureKind::Read), false) => {}
                (Some(_), true) | (Some(crate::ast::CaptureKind::Mut), false) => {
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
                        "nested function must explicitly capture '{name}' with {{...}}"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Record a name-based call as a capture when an enclosing policy permits
    /// it, without changing the historical rule that synthesized sibling calls
    /// may be reconstructed when no capture policy names them.
    pub(super) fn record_permitted_call_capture(&self, name: &str) {
        let contexts = self.capture_contexts.borrow();
        let Some(scope) = self.binding_scope(name) else {
            return;
        };
        if scope == 0 {
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
            let mut declarations = self.declaration_captures.borrow_mut();
            let captures = declarations.entry(policy.declaration.clone()).or_default();
            if !captures.iter().any(|capture| capture.binding == binding) {
                captures.push(checked);
            }
        }
    }

    pub(super) fn check_capture_capability(
        &self,
        name: &str,
        kind: crate::ast::CaptureKind,
    ) -> Result<(), TypeError> {
        let Some(ty) = self.lookup(name) else {
            return Ok(());
        };
        let missing = match kind {
            // `{var value}` performs an implicit copy at the nested-function
            // declaration. A merely explicitly Copyable value is therefore not
            // enough; current Mojo requires the stronger marker here.
            crate::ast::CaptureKind::Copy if !self.is_implicitly_copyable(ty) => {
                Some("ImplicitlyCopyable")
            }
            crate::ast::CaptureKind::Move if !self.is_movable(ty) => Some("Movable"),
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

    pub(super) fn lookup_owner(&self, name: &str) -> Option<crate::origin::OwnerId> {
        self.owner_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    pub(super) fn record_statement_binding(&self, statement: &Stmt, name: &str) {
        if let Some(owner) = self.lookup_owner(name) {
            self.statement_bindings
                .borrow_mut()
                .insert(statement.source_span(), owner);
        }
    }

    pub(super) fn owner_is_mutable(&self, owner: crate::origin::OwnerId) -> bool {
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
            .unwrap_or(false)
    }
}

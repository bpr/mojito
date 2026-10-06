//! The `infer_method_call` dispatcher.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl Checker {
    /// Infer a method call. An element of an unbound pack dispatches through
    /// its bounded view, and the result gets the dependent element back.
    pub(in crate::checker) fn infer_method_call(
        &self,
        span: &SourceSpan,
        object: &Expr,
        method: &str,
        call: MethodCallArguments<'_>,
    ) -> Result<Ty, TypeError> {
        self.infer_method_call_viewed(span, object, method, call)
            .map(|result| self.restore_pack_elements(result))
    }

    /// The effect-summary keys of every conformer's `method` a call through
    /// a parameter with `bounds` dispatches to, sorted: the whole-program
    /// dispatch set whose effects the call replays.
    pub(in crate::checker) fn dispatch_conformers(
        &self,
        bounds: &[String],
        method: &str,
    ) -> Vec<String> {
        let mut conformers: Vec<String> = self
            .structs
            .iter()
            .filter(|(_, info)| info.methods.contains_key(method))
            .filter(|(name, info)| {
                let implementation =
                    Ty::Struct((*name).clone(), params_as_args(&info.decls).into());
                bounds
                    .iter()
                    .all(|bound| self.conforms_to(&implementation, bound))
            })
            .flat_map(|(name, info)| {
                let signatures = &info.methods[method];
                if signatures.len() == 1 {
                    return vec![format!("{name}.{method}")];
                }
                let self_ty = Ty::Struct(name.clone(), params_as_args(&info.decls).into());
                signatures
                    .iter()
                    .map(|signature| method_lowered_name(name, method, signature, Some(&self_ty)))
                    .collect()
            })
            .collect();
        conformers.sort();
        conformers
    }

    /// Type a method call `object.method(args)`. On a generic struct value the
    /// method's parameter and return types are substituted at the receiver's
    /// type arguments; on a bounded type parameter (`x: T` with `T: SomeTrait`)
    /// the method is resolved from the bound trait's requirement, with `Self`
    /// substituted to `T`.
    fn infer_method_call_viewed(
        &self,
        span: &SourceSpan,
        object: &Expr,
        method: &str,
        call: MethodCallArguments<'_>,
    ) -> Result<Ty, TypeError> {
        let MethodCallArguments {
            args,
            kwargs,
            param_args,
            ..
        } = call;
        // `__mlir_op.`...`(...)` names no value: the one admitted operation
        // is a statement over a place.
        if matches!(&object.kind, ExprKind::Identifier(name) if name == "__mlir_op") {
            return self.infer_mlir_op(span, method, call);
        }
        if let Some(ty) = self.infer_type_receiver_call(span, object, method, call)? {
            return Ok(ty);
        }
        // Parameterless `__VariantStorage` operations
        // (`self._storage.deinit_with(handler)`) arrive as ordinary method
        // calls rather than parameterized invokes.
        if let Some(result) = self.infer_variant_storage_method(
            span.clone(),
            object,
            method,
            param_args,
            args,
            kwargs,
        ) {
            return result;
        }
        let obj_ty = self.infer(object)?;
        // An element of an unbound pack dispatches through its bounds.
        let obj_ty = self.opaque_element(&obj_ty).unwrap_or(obj_ty);
        // `Int.__mlir_index__()` is upstream's identity conversion to the
        // index type (spelled inside a user `Indexer`'s own
        // `__mlir_index__`); the VM represents the index as `Int`.
        if method == "__mlir_index__"
            && args.is_empty()
            && kwargs.is_empty()
            && param_args.is_empty()
            && matches!(obj_ty, Ty::Int | Ty::IntLiteral | Ty::UInt)
        {
            return Ok(Ty::Int);
        }
        // A receiver borrows a reference result for the call rather than
        // reading the referent out as an owned value; a consuming receiver
        // is gated on `ImplicitlyCopyable` below once the method resolves.
        if self.infer_reference_value(object).is_some() {
            self.borrowed_reference_receivers
                .borrow_mut()
                .insert(object.source_span());
        }
        let site = MethodCallSite {
            span,
            object,
            method,
            call,
            obj_ty: &obj_ty,
        };
        if let Some(ty) = self.infer_intrinsic_receiver_call(site)? {
            return Ok(ty);
        }
        let mut availability_failure = None;
        let selection = self.resolve_receiver_method(site, &mut availability_failure)?;
        match selected_method(method, selection, availability_failure)? {
            Some(resolved) => self.infer_selected_method_call(site, resolved),
            None => self.infer_unresolved_method(site),
        }
    }

    /// Type a call of the method `resolved` selected for the receiver at
    /// `site`: its clone when one serves the call, and otherwise the
    /// signature's effects, receiver and argument contracts, and result.
    pub(super) fn infer_selected_method_call(
        &self,
        site: MethodCallSite<'_>,
        resolved: MethodCallResolution,
    ) -> Result<Ty, TypeError> {
        let MethodCallSite {
            span,
            object,
            method,
            call,
            obj_ty,
        } = site;
        let MethodCallArguments {
            args,
            kwargs,
            parameterized_syntax,
            ..
        } = call;
        if let Some(ty) = self.retarget_to_method_clone(site, &resolved)? {
            return Ok(ty);
        }
        if parameterized_syntax {
            self.parameterized_method_calls
                .borrow_mut()
                .insert(span.clone(), resolved.param_decls.clone());
        }
        let boundary_before = self.call_boundary_snapshot(span, args, kwargs);
        self.record_selected_method_conversions(method, &resolved, args, kwargs)?;
        let call_error = resolved
            .raises
            .then(|| resolved.error.as_deref().cloned().unwrap_or(Ty::Error));
        if let Some(error) = &call_error {
            self.record_call_effect(span.clone(), error.clone());
            self.require_error(format!("call to raising method '{method}'"), error.clone())?;
        }
        let selected_target = resolved.lowered_name.clone().or_else(|| match obj_ty {
            Ty::Struct(name, _) if self.structs.contains_key(name) => {
                Some(format!("{name}.{method}"))
            }
            _ => None,
        });
        if let Some(target) = &selected_target {
            self.overload_targets
                .borrow_mut()
                .insert(span.clone(), target.clone());
        }
        self.replay_callee_effects(site, &resolved, selected_target.as_deref())?;
        self.check_mutated_receiver(site, &resolved)?;
        self.judge_parametric_origin_writes(site, &resolved)?;
        let effective_receiver_convention = self.effective_receiver_convention(site, &resolved)?;
        self.check_consumed_receiver(site, &resolved)?;
        let (effective_conventions, solved_return) =
            self.check_method_arguments(site, &resolved)?;
        let reference_result = self.method_reference_result(site, &resolved, solved_return)?;
        let boundary = self.checked_call_boundary(span, args, kwargs, &boundary_before);
        if let Some(target) = selected_target {
            self.record_selected_method_call(
                site,
                &resolved,
                SelectedMethodCall {
                    target,
                    raises: call_error,
                    receiver_convention: effective_receiver_convention,
                    conventions: effective_conventions,
                    reference_result: reference_result.clone(),
                    boundary,
                },
            );
        }
        Ok(reference_result.map_or_else(
            || {
                self.bind_call_result_tail(
                    span,
                    self.rebase_self_place_pointer(resolved.return_type, object),
                )
            },
            |reference| *reference.referent,
        ))
    }
}

/// The one signature a method selection settled on, `None` when the receiver
/// declares no method of the name, or why no candidate serves the call.
pub(super) fn selected_method(
    method: &str,
    selection: Result<Option<MethodCallResolution>, OverloadSelect>,
    availability_failure: Option<String>,
) -> Result<Option<MethodCallResolution>, TypeError> {
    match selection {
        Ok(resolved) => Ok(resolved),
        Err(OverloadSelect::NoMatch) => Err(TypeError::BadCall {
            func: method.to_string(),
            reason: availability_failure
                .unwrap_or_else(|| "no overload matches the supplied arguments".to_string()),
        }),
        Err(OverloadSelect::Ambiguous) => Err(TypeError::BadCall {
            func: method.to_string(),
            reason: "ambiguous overloaded method call".to_string(),
        }),
    }
}

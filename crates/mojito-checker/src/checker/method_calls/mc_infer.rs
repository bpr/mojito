//! The `infer_method_call` dispatcher.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

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
            .structs_declaring(method)
            .filter(|(name, info)| {
                let implementation =
                    Ty::new(TyKind::Struct((*name).clone(), params_as_args(&info.decls)));
                bounds
                    .iter()
                    .all(|bound| self.conforms_to(&implementation, bound))
            })
            .flat_map(|(name, info)| {
                let signatures = &info.methods[method];
                if signatures.len() == 1 {
                    return vec![format!("{name}.{method}")];
                }
                let self_ty = Ty::new(TyKind::Struct(name.clone(), params_as_args(&info.decls)));
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
        // `__mlir_op.`...`(...)` names no value: an ownership marker over a
        // place, or a `pop` or `index` op.
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
        // A tuple display received as a temporary holds its elements
        // materialized, as a binding of it does: `(1, 2)[0]` reads a
        // `Tuple[Int, Int]`.
        let obj_ty = match mojito_types::types::default_literal(&obj_ty) {
            materialized
                if matches!(object.kind, ExprKind::TupleLit(_)) && materialized != obj_ty =>
            {
                self.infer_with_expected(object, &materialized, true)?
            }
            _ => obj_ty,
        };
        // An element of an unbound pack dispatches through its bounds.
        let obj_ty = self.opaque_element(&obj_ty).unwrap_or(obj_ty);
        // `Int.__mlir_index__()` is upstream's `pop.cast_to_builtin` of the
        // `Int`'s storage to the builtin `index`, after a `UInt`'s
        // `pop.cast` to `Int` or a literal's materialization.
        if method == "__mlir_index__"
            && args.is_empty()
            && kwargs.is_empty()
            && param_args.is_empty()
            && matches!(
                obj_ty.kind(),
                TyKind::INT | TyKind::IntLiteral | TyKind::UINT
            )
        {
            return Ok(Ty::index());
        }
        // `Bool.__mlir_i1__()` is upstream's conversion to the builtin `i1`,
        // which is `Bool` while `Bool` is a compiler type.
        if method == "__mlir_i1__"
            && args.is_empty()
            && kwargs.is_empty()
            && param_args.is_empty()
            && matches!(obj_ty.kind(), TyKind::Bool)
        {
            return Ok(Ty::bool());
        }
        // `Bool.__mlir_bool__()` is upstream's read of its
        // `!kgen.scalar<bool>` storage, held as the one-lane `bool` vector
        // the `Bool` splats to while `Bool` is a compiler type.
        if method == "__mlir_bool__"
            && args.is_empty()
            && kwargs.is_empty()
            && param_args.is_empty()
            && matches!(obj_ty.kind(), TyKind::Bool)
        {
            return Ok(mojito_types::types::kgen_simd_ty(Dtype::Bool, 1));
        }
        // A heterogeneous collector's `get_loaded_kgen_pack()` is upstream's
        // `VariadicPack` loading its elements into a `!kgen.pack`, which is
        // the collector itself while the collector is compiler storage.
        if method == "get_loaded_kgen_pack"
            && args.is_empty()
            && kwargs.is_empty()
            && param_args.is_empty()
            && mojito_types::types::is_heterogeneous_collector(&obj_ty)
        {
            return Ok(obj_ty);
        }
        // A scalar's conversion dunders are the methods upstream's `SIMD`,
        // `Bool`, and numeric literals declare for `Boolable`, `Intable`,
        // and `Floatable`; the backends answer them through the conversion
        // built-in.
        if let Some((trait_name, result)) = match method {
            "__bool__" => Some(("Boolable", Ty::bool())),
            "__int__" => Some(("Intable", Ty::int())),
            "__float__" => Some(("Floatable", Ty::float64())),
            _ => None,
        } && args.is_empty()
            && kwargs.is_empty()
            && param_args.is_empty()
            && (is_numeric_like(&obj_ty) || is_scalar_simd(&obj_ty) || obj_ty == TyKind::Bool)
            && mojito_types::conformance::modelled_declares(&obj_ty, trait_name, &mut |_, _| true)
        {
            return Ok(result);
        }
        self.borrow_reference_receiver(object);
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

    /// Type operator dunder `method` on `object` as its method call
    /// `object.method(args)`, resolved the way upstream's `emitBinOpCall`
    /// and `UnaryOpNode::emitArith` resolve it, through the speculative
    /// `OverloadSet::lookupAndResolve`: `None`, recording nothing, when the
    /// receiver declares no such method or no candidate takes the operands,
    /// so the caller tries the reflected dunder or reports the operator.
    pub(in crate::checker) fn infer_operator_method_call(
        &self,
        span: &SourceSpan,
        object: &Expr,
        obj_ty: &Ty,
        method: &str,
        args: &[Expr],
    ) -> Result<Option<Ty>, TypeError> {
        let site = MethodCallSite {
            span,
            object,
            method,
            call: MethodCallArguments::ordinary(args, &[]),
            obj_ty,
        };
        let selection = match self.resolve_receiver_method(site, &mut None) {
            // A type parameter whose bounds declare no such requirement.
            Err(TypeError::NoSuchMethod { .. }) => return Ok(None),
            selection => selection?,
        };
        let resolved = match selection {
            Ok(Some(resolved)) => resolved,
            Ok(None) | Err(OverloadSelect::NoMatch) => return Ok(None),
            ambiguous @ Err(OverloadSelect::Ambiguous) => {
                return selected_method(method, ambiguous, None).map(|_| None);
            }
        };
        self.borrow_reference_receiver(object);
        self.infer_selected_method_call(site, resolved)
            .map(|result| Some(self.restore_pack_elements(result)))
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
        self.record_method_call_instantiation(site, &resolved);
        self.record_bracket_attributes(span.clone(), resolved.bracket_attributes.clone());
        if parameterized_syntax {
            self.record_at(span.clone()).parameterized_method_call =
                Some(resolved.param_decls.clone());
        }
        let boundary_before = self.call_boundary_snapshot(span, args, kwargs);
        self.record_selected_method_conversions(method, &resolved, args, kwargs)?;
        let call_error = resolved
            .raises
            .then(|| resolved.error.clone().unwrap_or_else(Ty::error));
        if let Some(error) = &call_error {
            self.record_call_effect(span.clone(), error.clone());
            self.require_error(format!("call to raising method '{method}'"), error.clone())?;
        }
        let selected_target = resolved
            .lowered_name
            .clone()
            .or_else(|| match obj_ty.kind() {
                TyKind::Struct(name, _) if self.structs.contains_key(name) => {
                    Some(format!("{name}.{method}"))
                }
                _ => self
                    .declared_receiver_member(obj_ty, method)
                    .map(|(name, _)| format!("{name}.{method}")),
            });
        if let Some(target) = &selected_target {
            self.record_at(span.clone()).overload_target = Some(target.clone());
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
            |reference| reference.referent,
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

impl Checker {
    /// A receiver borrows a reference result for the call rather than
    /// reading the referent out as an owned value; a consuming receiver is
    /// gated on `ImplicitlyCopyable` once the method resolves.
    fn borrow_reference_receiver(&self, object: &Expr) {
        if self.infer_reference_value(object).is_some() {
            self.borrowed_reference_receivers
                .borrow_mut()
                .insert(object.source_span());
        }
    }
}

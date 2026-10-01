//! Recording a selected method call's checked contract.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

/// What `infer_method_call` solved about a selected call beyond its
/// resolution, recorded into the checked call contract.
pub(super) struct SelectedMethodCall {
    pub(super) target: String,
    pub(super) raises: Option<Ty>,
    pub(super) receiver_convention: Option<ArgConvention>,
    pub(super) conventions: Vec<Option<ArgConvention>>,
    pub(super) reference_result: Option<mojito_types::origin::RefTy>,
    pub(super) boundary: mojito_checked::checked::CheckedCallBoundary,
}

impl Checker {
    /// Retain the complete selected-call payload independently of the
    /// compatibility adjustment slot. This is the authoritative handoff for
    /// nominal subscripts, and lets reference results coexist with descriptor
    /// and capture metadata at one source expression.
    pub(super) fn record_selected_method_call(
        &self,
        site: MethodCallSite<'_>,
        resolved: &MethodCallResolution,
        selected: SelectedMethodCall,
    ) {
        let MethodCallSite {
            span,
            object,
            method,
            call,
            obj_ty,
        } = site;
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            ..
        } = call;
        let SelectedMethodCall {
            target,
            raises,
            receiver_convention,
            conventions,
            reference_result,
            boundary,
        } = selected;
        let receiver_elided = self.receiver_is_elided(obj_ty, method, &target);
        let arguments = checked_call_arguments(resolved, &conventions);
        let argument_types = args
            .iter()
            .chain(kwargs.iter().map(|argument| &argument.value))
            .filter_map(|expression| {
                self.expression_types
                    .borrow()
                    .get(&expression.source_span())
                    .cloned()
            })
            .collect::<Vec<_>>();
        let captures = self.call_capture_effects(&argument_types);
        let parameter_arguments = self.checked_parameter_arguments(param_args);
        if reference_result.is_none() && !captures.is_empty() {
            self.operation_adjustments.borrow_mut().insert(
                span.clone(),
                mojito_checked::checked::SemanticAdjustment::CallableCaptureAccesses(
                    captures.clone(),
                ),
            );
        }
        let return_type = self.rebase_self_place_pointer(resolved.return_type.clone(), object);
        if reference_result.is_none() && captures.is_empty() {
            self.record_view_result(site, resolved, &return_type);
        }
        let return_type = self.bind_call_result_tail(span, return_type);
        self.call_parameters.borrow_mut().insert(
            span.clone(),
            resolved
                .parameter_names
                .iter()
                .zip(&resolved.conventions)
                .zip(&resolved.param_types)
                .map(|((name, convention), ty)| crate::checker::CallParameter {
                    name: name.clone(),
                    convention: *convention,
                    ty: ty.clone(),
                })
                .collect(),
        );
        self.selected_calls.borrow_mut().insert(
            span.clone(),
            mojito_checked::checked::CheckedCallContract {
                target,
                raises,
                result_ty: reference_result
                    .clone()
                    .map_or_else(|| return_type.clone(), Ty::Ref),
                result_adapter: resolved.result_adapter,
                receiver_requires_place: matches!(
                    resolved.self_convention,
                    Some(ArgConvention::Mut | ArgConvention::Ref)
                ),
                receiver_elided,
                receiver_convention,
                arguments,
                captures,
                reference_result,
                parameter_arguments,
                param_decls: resolved.param_decls.clone(),
                boundary,
            },
        );
    }

    /// A `@staticmethod` reached through an instance takes no receiver: the
    /// lowering evaluates the receiver for its effect and calls the static
    /// symbol with the arguments alone.
    fn receiver_is_elided(&self, obj_ty: &Ty, method: &str, target: &str) -> bool {
        match obj_ty {
            Ty::Struct(name, _) => self.structs.get(name).is_some_and(|info| {
                info.methods.get(method).is_some_and(|signatures| {
                    // The target spells the overload symbol only when
                    // the name is overloaded (see `lowered_name` above).
                    let self_ty = self.self_instance_ty(name);
                    signatures.iter().any(|sig| {
                        !sig.has_self
                            && (signatures.len() == 1
                                || method_lowered_name(name, method, sig, self_ty.as_ref())
                                    == target)
                    })
                })
            }),
            _ => false,
        }
    }

    /// The call's explicit compile-time arguments, each naming its value
    /// source unless the argument is erased.
    fn checked_parameter_arguments(
        &self,
        param_args: &[mojito_ast::ast::ParamArg],
    ) -> Vec<mojito_checked::checked::CheckedCallParameterArgument> {
        param_args
            .iter()
            .filter_map(|argument| {
                let (name, argument) = match argument {
                    mojito_ast::ast::ParamArg::Named { name, value } => {
                        (Some(name.clone()), value.as_ref())
                    }
                    argument => (None, argument),
                };
                let value_source = match argument {
                    mojito_ast::ast::ParamArg::Type(_) => None,
                    mojito_ast::ast::ParamArg::Value(expression) => {
                        let erased = self
                            .operation_adjustments
                            .borrow()
                            .get(&expression.source_span())
                            .is_some_and(|adjustment| {
                                matches!(
                                    adjustment,
                                    mojito_checked::checked::SemanticAdjustment::EraseCompileTimeArgument
                                        | mojito_checked::checked::SemanticAdjustment::ReifyTypeArgument {
                                            ..
                                        }
                                )
                            });
                        if erased {
                            return None;
                        }
                        Some(expression.source_span())
                    }
                    mojito_ast::ast::ParamArg::Named { .. } => unreachable!(),
                };
                Some(mojito_checked::checked::CheckedCallParameterArgument { name, value_source })
            })
            .collect()
    }

    /// A method whose non-consuming receiver hands back a ref-field
    /// struct (a borrowing view/iterator) lends the receiver to the
    /// result, exactly as a view-typed subscript does: the loan keeps
    /// the source alive while the view does and rejects source
    /// mutation. Capture-carrying calls keep their capture adjustment;
    /// reference results already carry their own loan channel. A
    /// result whose loans arrive only through a type argument
    /// (`List[Span[Int, o]].pop()`) is an element, not a view.
    fn record_view_result(
        &self,
        site: MethodCallSite<'_>,
        resolved: &MethodCallResolution,
        return_type: &Ty,
    ) {
        let MethodCallSite {
            span,
            object,
            call,
            obj_ty,
            ..
        } = site;
        let MethodCallArguments { args, kwargs, .. } = call;
        let lends_receiver = !resolved.consumes_receiver
            && matches!(return_type, Ty::Struct(..))
            && self.type_carries_loans(return_type)
            && resolved
                .declared_return
                .as_ref()
                .is_none_or(|declared| self.type_carries_loans(declared));
        if !lends_receiver {
            return;
        }
        self.operation_adjustments
            .borrow_mut()
            .entry(span.clone())
            .or_insert(
                mojito_checked::checked::SemanticAdjustment::BorrowViewResult {
                    materialized: None,
                },
            );
        if !resolved.view_return_interior.is_empty() {
            self.view_result_interiors
                .borrow_mut()
                .insert(span.clone(), resolved.view_return_interior.clone());
        }
        // An owning temporary receiver (`String("abc").codepoints()`)
        // has no place to lend, so materialize it as an anonymous
        // owned binding: the hidden slot gives the loan a real place
        // and the temporary its borrower's lifetime, exactly as a
        // temporary bound to a `ref [origin]` parameter gets one. A
        // borrowing temporary receiver (`StringSpan(s).split(",")`)
        // already anchors on its own loans. A rejection leaves the
        // receiver alone: another adjustment owns its lowering
        // contract.
        if !crate::checker::places::is_place_expr(object) && !self.type_carries_loans(obj_ty) {
            let _ = self.materialize_borrow_owner(object, resolved.mutates_receiver);
        }
        self.record_call_result_origins(
            span,
            &resolved.view_return,
            &resolved.slots,
            args,
            kwargs,
            Some(object),
        );
    }
}

/// The selected signature's arguments in slot order, then the variadic and
/// keyword overflow.
fn checked_call_arguments(
    resolved: &MethodCallResolution,
    effective_conventions: &[Option<ArgConvention>],
) -> Vec<mojito_checked::checked::CheckedCallArgument> {
    use mojito_checked::checked::{CheckedCallArgument, CheckedCallArgumentSource};
    let mut arguments = resolved
        .slots
        .iter()
        .enumerate()
        .map(|(index, slot)| CheckedCallArgument {
            source: match slot {
                ArgSlot::Positional(position) => CheckedCallArgumentSource::Positional(*position),
                ArgSlot::Keyword(position) => CheckedCallArgumentSource::Keyword(*position),
                ArgSlot::Default => CheckedCallArgumentSource::Default,
            },
            parameter_ty: resolved
                .param_types
                .get(index)
                .cloned()
                .unwrap_or(Ty::Error),
            requires_place: matches!(
                resolved.conventions.get(index).copied().flatten(),
                Some(ArgConvention::Mut | ArgConvention::Ref)
            ),
            convention: effective_conventions.get(index).copied().flatten(),
        })
        .collect::<Vec<_>>();
    if let Some(element) = &resolved.variadic_element {
        arguments.extend(resolved.positional_overflow.iter().enumerate().map(
            |(pack_index, position)| CheckedCallArgument {
                source: CheckedCallArgumentSource::Positional(*position),
                parameter_ty: match element {
                    Ty::RuntimePack(elements) => {
                        elements.get(pack_index).cloned().unwrap_or(Ty::Error)
                    }
                    _ => element.clone(),
                },
                requires_place: false,
                convention: None,
            },
        ));
    }
    if let Some(element) = &resolved.keyword_element {
        arguments.extend(
            resolved
                .keyword_overflow
                .iter()
                .map(|position| CheckedCallArgument {
                    source: CheckedCallArgumentSource::Keyword(*position),
                    parameter_ty: element.clone(),
                    requires_place: false,
                    convention: None,
                }),
        );
    }
    arguments
}

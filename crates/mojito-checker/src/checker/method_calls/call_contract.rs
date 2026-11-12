//! Recording a selected method call's checked contract.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

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
        // A capturing closure bound to a compile-time callable parameter
        // runs while the call is active, as a runtime callable argument does,
        // and a closure's own `__call__` runs it, as a call through it does.
        let called_closure =
            (method == "__call__" && is_closure_storage(obj_ty)).then(|| obj_ty.clone());
        let attributes = self.recorded_bracket_attributes(span);
        let argument_types = called_closure
            .into_iter()
            .chain(
                super::super::runtime_param_arg_expressions(param_args, &attributes)
                    .into_iter()
                    .chain(args)
                    .chain(kwargs.iter().map(|argument| &argument.value))
                    .filter_map(|expression| {
                        self.fact(&expression.source_span(), |facts| facts.ty.as_ref())
                    }),
            )
            .collect::<Vec<_>>();
        let captures = self.call_capture_effects(&argument_types);
        let parameter_arguments = checked_parameter_arguments(
            param_args,
            &self
                .fact(span, |facts| facts.parameter_attributes.as_ref())
                .unwrap_or_default(),
        );
        if reference_result.is_none() && !captures.is_empty() {
            self.record_capture_accesses(span.clone(), &captures);
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
        if reference_result.is_none() {
            self.record_at(span.clone())
                .declared_result
                .clone_from(&resolved.declared_return);
        }
        self.record_at(span.clone()).selected_call =
            Some(Box::new(mojito_checked::checked::CheckedCallContract {
                target,
                raises,
                result_ty: reference_result.clone().map_or_else(
                    || return_type.clone(),
                    |reference| TyKind::Ref(reference).into(),
                ),
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
                refined_bound: resolved.refined_bound.clone(),
            }));
    }

    /// A `@staticmethod` reached through an instance takes no receiver: the
    /// lowering evaluates the receiver for its effect and calls the static
    /// symbol with the arguments alone.
    fn receiver_is_elided(&self, obj_ty: &Ty, method: &str, target: &str) -> bool {
        match self.nominal_struct(obj_ty) {
            Some((name, _)) => self.structs.get(name).is_some_and(|info| {
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
            None => false,
        }
    }

    /// A method whose non-consuming receiver hands back a ref-field
    /// struct (a borrowing view/iterator), or an associated projection
    /// applied at an origin (`C.IteratorType[origin_of(items)]`), lends
    /// the receiver to the result, exactly as a view-typed subscript does: the loan keeps
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
            && match return_type.kind() {
                struct_ty if let Some(_) = struct_ty.as_non_simd_struct() => {
                    self.type_carries_loans(return_type)
                        && resolved
                            .declared_return
                            .as_ref()
                            .is_none_or(|declared| self.type_carries_loans(declared))
                }
                TyKind::Assoc { args, .. } => args
                    .iter()
                    .any(|argument| matches!(argument, TyArg::Origin(_))),
                _ => false,
            };
        if !lends_receiver {
            return;
        }
        self.record_at(span.clone())
            .adjustment
            .get_or_insert_with(|| {
                Box::new(
                    mojito_checked::checked::SemanticAdjustment::BorrowViewResult {
                        materialized: None,
                        interior: resolved.view_return_interior.clone(),
                    },
                )
            });
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
        if !self.designates_place(object) && !self.type_carries_loans(obj_ty) {
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
                .unwrap_or_else(Ty::error),
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
                parameter_ty: match element.kind() {
                    TyKind::RuntimePack(elements) => {
                        elements.get(pack_index).cloned().unwrap_or_else(Ty::error)
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

/// The call's explicit compile-time arguments, one parameter per declared
/// slot of `binding` (the binding its binder recorded, keyed by
/// declaration), then one per argument bound to none. A value argument
/// bound to no attribute names its run-time operand by its source; one
/// bound to an origin is left out: it fills the callee's origin slot,
/// which no function declaration lists.
fn checked_parameter_arguments(
    param_args: &[mojito_ast::ast::ParamArg],
    binding: &mojito_checked::checked::BracketBinding,
) -> Vec<mojito_checked::checked::CheckedCallParameter> {
    use mojito_checked::checked::{CheckedCallParameter, CheckedCallParameterArgument};
    let argument = |argument: &mojito_ast::ast::ParamArg, attribute: Option<&TyArg>| {
        let argument = match argument {
            mojito_ast::ast::ParamArg::Named { value, .. } => &**value,
            argument => argument,
        };
        let operand = match (argument, attribute) {
            (_, Some(TyArg::Origin(_))) => return None,
            (mojito_ast::ast::ParamArg::Value(value), None) => Some(value.source_span()),
            _ => None,
        };
        Some(CheckedCallParameterArgument { operand })
    };
    let mut placed = vec![false; param_args.len()];
    let mut parameters = binding
        .slots
        .iter()
        .map(|slot| CheckedCallParameter {
            arguments: slot
                .iter()
                .filter(|bound| {
                    bound.position < placed.len()
                        && !std::mem::replace(&mut placed[bound.position], true)
                })
                .filter_map(|bound| argument(&param_args[bound.position], bound.attribute.as_ref()))
                .collect(),
        })
        .collect::<Vec<_>>();
    parameters.extend(
        param_args
            .iter()
            .zip(placed)
            .filter(|(_, placed)| !placed)
            .filter_map(|(param_arg, _)| argument(param_arg, None))
            .map(|argument| CheckedCallParameter {
                arguments: vec![argument],
            }),
    );
    parameters
}

//! Iterator-protocol selection for `for`/comprehension iterables and the
//! `for ref` bridge. Extracted from `checker.rs`; see `docs/symbol-map.md`.

use super::*;

impl Checker {
    /// Select source ownership independently from the loop target convention.
    /// A top-level transfer is the source-language request for consuming
    /// `__iter__(var self)`; every other expression uses borrowed iteration.
    pub(super) fn iteration_mode(iter: &Expr) -> crate::checked::IterationMode {
        if matches!(iter.kind, ExprKind::Transfer(_)) {
            crate::checked::IterationMode::Owned
        } else {
            crate::checked::IterationMode::Borrowed
        }
    }

    /// Combine a target convention with the exact checked `__next__` result.
    /// This is the single matrix shared by statements and comprehensions.
    pub(super) fn iteration_binding_plan(
        &mut self,
        mode: crate::ast::LoopBindingMode,
        yielded_ty: &Ty,
    ) -> Result<crate::checked::CheckedIterationBinding, TypeError> {
        use crate::ast::LoopBindingMode;
        use crate::checked::{CheckedIterationBinding, IterationBindingAction};
        use crate::origin::Mutability;

        if let Ty::Ref(reference) = yielded_ty {
            return match mode {
                LoopBindingMode::Immutable => {
                    let mut binding = reference.clone();
                    binding.mutability = Mutability::Immutable;
                    Ok(CheckedIterationBinding {
                        mode,
                        action: IterationBindingAction::BorrowReference,
                        yielded_ty: yielded_ty.clone(),
                        binding_ty: Ty::Ref(binding),
                        mutable: false,
                    })
                }
                LoopBindingMode::Var => {
                    if !self.is_implicitly_copyable(&reference.referent) {
                        return Err(TypeError::TraitNotSatisfied {
                            param: "loop item".to_string(),
                            ty: reference.referent.to_string(),
                            trait_name: "ImplicitlyCopyable".to_string(),
                            reason: self
                                .trait_failure_reason(&reference.referent, "ImplicitlyCopyable"),
                        });
                    }
                    Ok(CheckedIterationBinding {
                        mode,
                        action: IterationBindingAction::CopyReference,
                        yielded_ty: yielded_ty.clone(),
                        binding_ty: (*reference.referent).clone(),
                        mutable: true,
                    })
                }
                LoopBindingMode::Ref => Ok(CheckedIterationBinding {
                    mode,
                    action: IterationBindingAction::BorrowReference,
                    yielded_ty: yielded_ty.clone(),
                    binding_ty: yielded_ty.clone(),
                    mutable: reference.mutability == Mutability::Mutable,
                }),
            };
        }

        if !self.is_movable(yielded_ty) {
            return Err(TypeError::TraitNotSatisfied {
                param: "loop item".to_string(),
                ty: yielded_ty.to_string(),
                trait_name: "Movable".to_string(),
                reason: self.trait_failure_reason(yielded_ty, "Movable"),
            });
        }
        if mode == LoopBindingMode::Var {
            return Ok(CheckedIterationBinding {
                mode,
                action: IterationBindingAction::MoveValue,
                yielded_ty: yielded_ty.clone(),
                binding_ty: yielded_ty.clone(),
                mutable: true,
            });
        }

        // An immutable/`ref` target over an rvalue result is a borrow of the
        // freshly yielded value: it lives in the loop variable's own
        // per-iteration storage and is dropped at the end of each iteration
        // rather than moved onward, so it must be droppable. Unlike a `var`
        // target it cannot be transferred out when immutable. Because `__next__`
        // already produced an owned value with no external referent, the binding
        // stores that value directly instead of an alias — a synthetic reference
        // into per-iteration storage would dangle on early control-flow exits.
        // A `Copyable` element is droppable even without an explicit
        // `ImplicitlyDeletable` bound (its copies must be destroyed), so it also
        // qualifies; only a linear element with no implicit destructor is
        // rejected.
        if !self.is_implicitly_deletable(yielded_ty) && !self.is_copyable(yielded_ty) {
            return Err(TypeError::TraitNotSatisfied {
                param: "loop item".to_string(),
                ty: yielded_ty.to_string(),
                trait_name: "ImplicitlyDeletable".to_string(),
                reason: self.trait_failure_reason(yielded_ty, "ImplicitlyDeletable"),
            });
        }
        Ok(CheckedIterationBinding {
            mode,
            action: IterationBindingAction::BorrowValue,
            yielded_ty: yielded_ty.clone(),
            binding_ty: yielded_ty.clone(),
            mutable: mode == LoopBindingMode::Ref,
        })
    }

    /// Resolve the exact methods used by the bundled List `for ref` bridge.
    /// These synthetic calls are checked here and retained on the iteration
    /// protocol; their syntax is never added to the source-expression arena.
    pub(super) fn reference_iteration_protocol(
        &self,
        object: &Expr,
    ) -> Result<crate::checked::ReferenceIterationProtocol, TypeError> {
        let call_site = || {
            let mut expression = Expr::new(ExprKind::None, crate::token::DUMMY_SPAN);
            expression.source = object.source.clone();
            expression.source_span()
        };
        let len_site = call_site();
        self.infer_method_call(
            len_site.clone(),
            object,
            "__len__",
            MethodCallArguments::ordinary(&[], &[]),
        )?;
        let len = self
            .selected_calls
            .borrow_mut()
            .remove(&len_site)
            .ok_or_else(|| {
                TypeError::InvariantViolation(
                    "List reference iteration lost its selected __len__ contract".to_string(),
                )
            })?;

        let mut index = Expr::new(ExprKind::Int(0i64.into()), crate::token::DUMMY_SPAN);
        index.source = object.source.clone();
        let getitem_site = call_site();
        self.infer_method_call(
            getitem_site.clone(),
            object,
            "__getitem__",
            MethodCallArguments::ordinary(std::slice::from_ref(&index), &[]),
        )?;
        let getitem = self
            .selected_calls
            .borrow_mut()
            .remove(&getitem_site)
            .ok_or_else(|| {
                TypeError::InvariantViolation(
                    "List reference iteration lost its selected __getitem__ contract".to_string(),
                )
            })?;
        if getitem.reference_result.is_none() {
            return Err(TypeError::InvariantViolation(
                "List reference iteration requires a reference-returning __getitem__".to_string(),
            ));
        }
        Ok(crate::checked::ReferenceIterationProtocol { len, getitem })
    }

    /// Resolve a loop's complete iterator protocol.  In particular, owned
    /// iteration selects `__iter__(var self)` and never silently falls back to a
    /// borrowed `__iter__`.  The selected symbols cross the checked boundary so
    /// HIR/MIR/VM do not repeat overload selection.
    pub(super) fn iteration_protocol(
        &self,
        ty: &Ty,
        mode: crate::checked::IterationMode,
    ) -> Result<(Ty, crate::checked::IterationProtocol), TypeError> {
        use crate::checked::{IterationMode, IterationProtocol};
        let builtin = |element| {
            (
                element,
                IterationProtocol {
                    mode,
                    binding: None,
                    borrowed_origin: None,
                    reference: None,
                    prepare: Vec::new(),
                    has_next: None,
                    next: None,
                    exhaustion: None,
                },
            )
        };
        // Focused checker users may deliberately omit the implicit prelude.
        // Preserve the old intrinsic proof only in that compatibility mode;
        // linked production programs have registered nominal declarations and
        // must resolve their ordinary `__iter__`/`__next__` contracts below.
        if let Ty::Struct(name, _) = ty
            && !self.structs.contains_key(name)
        {
            if crate::types::is_range_type(ty) {
                return Ok(builtin(Ty::Int));
            }
            if let Some(element) = list_element(ty).or_else(|| set_element(ty)) {
                return Ok(builtin(element.clone()));
            }
            if let Some((key, _)) = dict_elements(ty) {
                return Ok(builtin(key.clone()));
            }
        }
        match ty {
            Ty::VariadicPack(element) => Ok(builtin((**element).clone())),
            Ty::Struct(..) => self.struct_iteration_protocol(ty, mode, 0),
            Ty::Param { bounds, .. } => {
                let owned = mode == IterationMode::Owned;
                let required = if owned { "IterableOwned" } else { "Iterable" };
                if !bounds.iter().any(|bound| bound == required)
                    && self.lookup_trait_assoc_type(bounds, "Element").is_none()
                {
                    return Err(TypeError::TypeMismatch {
                        expected: format!("a type conforming to {required}"),
                        found: ty.to_string(),
                        context: "for-loop iterable".to_string(),
                    });
                }
                if owned && !bounds.iter().any(|bound| bound == "IterableOwned") {
                    return Err(TypeError::TraitNotSatisfied {
                        param: "T".to_string(),
                        ty: ty.to_string(),
                        trait_name: "IterableOwned".to_string(),
                        reason: Some(
                            "owned iteration requires an ownership-consuming iterator".to_string(),
                        ),
                    });
                }
                let element = Ty::Assoc {
                    base: Box::new(ty.clone()),
                    name: "Element".to_string(),
                    args: Vec::new(),
                };
                Ok((
                    element.clone(),
                    IterationProtocol {
                        mode,
                        binding: None,
                        borrowed_origin: None,
                        reference: None,
                        prepare: vec![crate::symbol::iterator_dispatch_symbol(match mode {
                            IterationMode::Borrowed => crate::ast::ArgConvention::Read,
                            IterationMode::Owned => crate::ast::ArgConvention::Var,
                        })],
                        has_next: Some("__iterator_dispatch.__len__".to_string()),
                        next: Some(Box::new(crate::checked::CheckedIteratorCall {
                            target: "__iterator_dispatch.__next__".to_string(),
                            result_ty: element,
                            reference_result: None,
                            raises: None,
                            result_adapter: Some(
                                crate::checked::CheckedResultAdapter::CopyIteratorReference,
                            ),
                        })),
                        exhaustion: None,
                    },
                ))
            }
            other => Err(TypeError::TypeMismatch {
                expected: if mode == IterationMode::Owned {
                    "a nominal collection or a type with __iter__(var self)"
                } else {
                    "a nominal collection or a type with borrowed __iter__"
                }
                .to_string(),
                found: other.to_string(),
                context: "for-loop iterable".to_string(),
            }),
        }
    }

    pub(super) fn struct_iteration_protocol(
        &self,
        c_ty: &Ty,
        mode: crate::checked::IterationMode,
        depth: usize,
    ) -> Result<(Ty, crate::checked::IterationProtocol), TypeError> {
        use crate::checked::IterationMode;
        let no_method = |ty: &Ty, m: &str| TypeError::NoSuchMethod {
            object_type: ty.to_string(),
            method: m.to_string(),
        };
        if depth >= 8 {
            return Err(TypeError::Unsupported(
                "iterator normalization exceeded eight __iter__ steps".to_string(),
            ));
        }
        let Ty::Struct(cname, ctargs) = c_ty else {
            return Err(no_method(c_ty, "__iter__"));
        };
        let cinfo = self.structs.get(cname).ok_or_else(|| {
            TypeError::InvariantViolation(format!("struct '{cname}' was not registered"))
        })?;
        let candidates = cinfo
            .methods
            .get("__iter__")
            .ok_or_else(|| no_method(c_ty, "__iter__"))?;
        let matching = candidates
            .iter()
            .filter(|sig| match mode {
                IterationMode::Owned => sig.self_convention == Some(crate::ast::ArgConvention::Var),
                IterationMode::Borrowed => matches!(
                    sig.self_convention,
                    None | Some(crate::ast::ArgConvention::Read | crate::ast::ArgConvention::Ref)
                ),
            })
            .filter_map(|sig| {
                self.instantiate_iteration_method(cname, cinfo, ctargs, sig)
                    .map(|(ret, _, error)| (sig, ret, error))
            })
            .collect::<Vec<_>>();
        let [(iter_sig, it_ty, iter_error)] = matching.as_slice() else {
            if matching.len() > 1 {
                return Err(TypeError::BadCall {
                    func: format!("{cname}.__iter__"),
                    reason: "ambiguous iterator receiver convention".to_string(),
                });
            }
            return Err(TypeError::TypeMismatch {
                expected: match mode {
                    IterationMode::Owned => "an '__iter__(var self)' method",
                    IterationMode::Borrowed => "a borrowed '__iter__' method",
                }
                .to_string(),
                found: format!("{}.__iter__", c_ty),
                context: "for-loop iterator selection".to_string(),
            });
        };
        if let Some(error) = iter_error {
            self.require_error(
                format!("implicit call to raising method '{cname}.__iter__'"),
                error.clone(),
            )?;
        }
        let prepare_symbol = if candidates.len() > 1 {
            method_lowered_name(cname, "__iter__", iter_sig)
        } else {
            format!("{cname}.__iter__")
        };
        // The iterator must itself be a struct with `__next__`. Current Mojo
        // terminates iteration when that method raises the typed
        // `StopIteration`; the legacy bounded protocol additionally exposes
        // `__len__` and keeps the old nonraising `__next__` path available.
        let bad_iter = || TypeError::TypeMismatch {
            expected: "List or an iterator struct with __next__".to_string(),
            found: it_ty.to_string(),
            context: "__iter__ return type".to_string(),
        };
        let Ty::Struct(iname, itargs) = it_ty else {
            return Err(bad_iter());
        };
        let iinfo = self.structs.get(iname).ok_or_else(bad_iter)?;
        if !iinfo.methods.contains_key("__next__") && iinfo.methods.contains_key("__iter__") {
            let (element, mut nested) = self.struct_iteration_protocol(it_ty, mode, depth + 1)?;
            nested.prepare.insert(0, prepare_symbol);
            return Ok((element, nested));
        }
        // `__next__(mut self)` advances, so it must mutate `self`.
        let next_candidates = iinfo
            .methods
            .get("__next__")
            .ok_or_else(|| no_method(it_ty, "__next__"))?;
        let applicable_next = next_candidates
            .iter()
            .filter_map(|sig| {
                self.instantiate_iteration_method(iname, iinfo, itargs, sig)
                    .map(|(ret, reference, error)| (sig, ret, reference, error))
            })
            .collect::<Vec<_>>();
        let [(next_sig, element, reference_result, next_error)] = applicable_next.as_slice() else {
            return Err(no_method(it_ty, "__next__"));
        };
        if !matches!(
            next_sig.self_convention,
            Some(crate::ast::ArgConvention::Mut)
        ) {
            return Err(TypeError::TypeMismatch {
                expected: "a 'mut self' __next__".to_string(),
                found: "read-only self".to_string(),
                context: "iterator '__next__'".to_string(),
            });
        }
        let next_symbol = if iinfo
            .methods
            .get("__next__")
            .is_some_and(|methods| methods.len() > 1)
        {
            method_lowered_name(iname, "__next__", next_sig)
        } else {
            format!("{iname}.__next__")
        };
        let checked_next = crate::checked::CheckedIteratorCall {
            target: next_symbol,
            result_ty: element.clone(),
            reference_result: reference_result.clone(),
            raises: next_error.clone(),
            result_adapter: None,
        };
        if next_sig.raises {
            let exhaustion = next_error.clone().unwrap_or(Ty::Error);
            let is_stop_iteration = matches!(
                &exhaustion,
                Ty::Struct(name, arguments)
                    if arguments.is_empty()
                        && (name == "StopIteration" || name.ends_with("$StopIteration"))
            );
            if !is_stop_iteration {
                return Err(TypeError::TypeMismatch {
                    expected: "an '__next__' that raises StopIteration".to_string(),
                    found: format!("raises {exhaustion}"),
                    context: "iterator '__next__' exhaustion contract".to_string(),
                });
            }
            return Ok((
                element.clone(),
                crate::checked::IterationProtocol {
                    mode,
                    binding: None,
                    borrowed_origin: None,
                    reference: None,
                    prepare: vec![prepare_symbol],
                    has_next: None,
                    next: Some(Box::new(checked_next)),
                    exhaustion: Some(exhaustion),
                },
            ));
        }

        // Backward-compatible bounded iteration: `__len__(self) -> Int`
        // determines whether the nonraising `__next__` may be called.
        let len_candidates = iinfo
            .methods
            .get("__len__")
            .ok_or_else(|| no_method(it_ty, "__len__"))?;
        let applicable_len = len_candidates
            .iter()
            .filter_map(|sig| {
                self.instantiate_iteration_method(iname, iinfo, itargs, sig)
                    .map(|(ret, _, _)| (sig, ret))
            })
            .collect::<Vec<_>>();
        let [(len_sig, len_ret)] = applicable_len.as_slice() else {
            return Err(no_method(it_ty, "__len__"));
        };
        if *len_ret != Ty::Int {
            return Err(TypeError::TypeMismatch {
                expected: "Int".to_string(),
                found: len_ret.to_string(),
                context: "return type of iterator '__len__'".to_string(),
            });
        }
        Ok((
            element.clone(),
            crate::checked::IterationProtocol {
                mode,
                binding: None,
                borrowed_origin: None,
                reference: None,
                prepare: vec![prepare_symbol],
                has_next: Some(
                    if iinfo
                        .methods
                        .get("__len__")
                        .is_some_and(|methods| methods.len() > 1)
                    {
                        method_lowered_name(iname, "__len__", len_sig)
                    } else {
                        format!("{iname}.__len__")
                    },
                ),
                next: Some(Box::new(checked_next)),
                exhaustion: None,
            },
        ))
    }

    /// Instantiate one nullary iterator-protocol method exactly as an ordinary
    /// method call would. In particular, a method-level `where` clause may name
    /// either its own compile-time parameters or the receiver struct's
    /// parameters (`Self.T` is canonicalized to `T`). A declaration which is
    /// present by name but unavailable for this specialization is not a protocol
    /// implementation.
    pub(super) fn instantiate_iteration_method(
        &self,
        owner: &str,
        info: &StructInfo,
        receiver_arguments: &[TyArg],
        signature: &MethodSig,
    ) -> Option<(Ty, Option<crate::origin::RefTy>, Option<Ty>)> {
        if !signature.has_self || !signature.params.is_empty() {
            return None;
        }
        let receiver_subst = struct_subst(&info.decls, receiver_arguments);
        let params = signature
            .params
            .iter()
            .map(|ty| substitute(ty, &receiver_subst))
            .collect::<Vec<_>>();
        let receiver_variadic = signature
            .variadic
            .as_deref()
            .map(|ty| substitute(ty, &receiver_subst));
        let receiver_kw_variadic = signature
            .kw_variadic
            .as_deref()
            .map(|ty| substitute(ty, &receiver_subst));
        let (_, variadic, kw_variadic, method_subst, mut arguments) = self
            .instantiate_method_generics(
                &format!("{owner} iterator protocol"),
                signature,
                &params,
                receiver_variadic.as_ref(),
                receiver_kw_variadic.as_ref(),
                &[],
                &[],
                &[],
            )
            .ok()?;
        let method_arguments = arguments.clone();
        // Iterator dunders have no explicit runtime arguments. A variadic or
        // keyword-variadic declaration is not the exact protocol shape even
        // though an empty ordinary call could technically invoke it.
        if variadic.is_some() || kw_variadic.is_some() {
            return None;
        }
        for (decl, argument) in info.decls.iter().zip(receiver_arguments) {
            arguments.insert(
                decl.name().trim_start_matches('*').to_string(),
                argument.clone(),
            );
        }
        if !self.method_constraints_apply(signature, &arguments) {
            return None;
        }
        let instantiate = |ty: &Ty| substitute(&substitute(ty, &receiver_subst), &method_subst);
        let referent = instantiate(&signature.ret);
        let mut semantic_arguments = receiver_arguments.to_vec();
        semantic_arguments.extend(signature.decls.iter().filter_map(|declaration| {
            method_arguments
                .get(declaration.name().trim_start_matches('*'))
                .cloned()
        }));
        let reference_result = signature.ref_return.as_ref().map(|reference| {
            instantiate_iterator_reference(
                reference,
                referent.clone(),
                &semantic_arguments,
                signature.self_convention == Some(crate::ast::ArgConvention::Mut),
            )
        });
        let result = reference_result
            .as_ref()
            .map_or_else(|| referent.clone(), |reference| Ty::Ref(reference.clone()));
        Some((
            result,
            reference_result,
            signature.raises.then(|| {
                signature
                    .error
                    .as_deref()
                    .map(instantiate)
                    .unwrap_or(Ty::Error)
            }),
        ))
    }
}

fn instantiate_iterator_reference(
    signature: &crate::origin::RefSig,
    referent: Ty,
    arguments: &[TyArg],
    mutable_receiver: bool,
) -> crate::origin::RefTy {
    use crate::origin::{Mutability, RefTy, SigMutability};

    let origin = instantiate_iterator_sig_origin(&signature.origin, arguments);
    let mutability = match signature.mutability {
        SigMutability::Immutable => Mutability::Immutable,
        SigMutability::Mutable => Mutability::Mutable,
        SigMutability::BoolParam(index) => match iterator_bool_argument(arguments, index) {
            Some(true) => Mutability::Mutable,
            Some(false) => Mutability::Immutable,
            None => Mutability::Param(crate::origin::OriginParamId(index as u32)),
        },
        SigMutability::Infer => match &origin {
            crate::origin::Origin::Static => Mutability::Immutable,
            crate::origin::Origin::Untracked { mutable: true } => Mutability::Mutable,
            crate::origin::Origin::Untracked { mutable: false } => Mutability::Immutable,
            crate::origin::Origin::Param(parameter) => Mutability::Param(*parameter),
            _ if mutable_receiver => Mutability::Mutable,
            _ => Mutability::Immutable,
        },
    };
    RefTy {
        referent: Box::new(referent),
        origin,
        mutability,
    }
}

fn instantiate_iterator_sig_origin(
    signature: &crate::origin::SigOrigin,
    arguments: &[TyArg],
) -> crate::origin::Origin {
    use crate::origin::{Origin, OriginParamId, SigOrigin};

    match signature {
        SigOrigin::Self_ | SigOrigin::Infer => Origin::SelfParam,
        SigOrigin::Param(index) => Origin::Param(OriginParamId(*index as u32)),
        SigOrigin::Bound(origin) => instantiate_iterator_bound_origin(origin, arguments),
        SigOrigin::Static => Origin::Static,
        SigOrigin::Untracked { mutable } => Origin::Untracked { mutable: *mutable },
        SigOrigin::Projected(base, path) => crate::checker::origins::project_origin(
            instantiate_iterator_sig_origin(base, arguments),
            path,
        ),
        SigOrigin::Union(members) => Origin::union(
            members
                .iter()
                .map(|member| instantiate_iterator_sig_origin(member, arguments)),
        ),
    }
}

fn instantiate_iterator_bound_origin(
    origin: &crate::origin::Origin,
    arguments: &[TyArg],
) -> crate::origin::Origin {
    use crate::origin::Origin;

    match origin {
        Origin::Param(parameter) => iterator_origin_argument(arguments, parameter.0 as usize)
            .unwrap_or(Origin::Param(*parameter)),
        Origin::Union(members) => Origin::union(
            members
                .iter()
                .map(|member| instantiate_iterator_bound_origin(member, arguments)),
        ),
        _ => origin.clone(),
    }
}

fn iterator_origin_argument(arguments: &[TyArg], index: usize) -> Option<crate::origin::Origin> {
    if let Some(TyArg::Origin(origin)) = arguments.get(index) {
        return Some(origin.clone());
    }
    let mut origins = arguments.iter().filter_map(|argument| match argument {
        TyArg::Origin(origin) => Some(origin),
        TyArg::Ty(_) | TyArg::Val(_) => None,
    });
    let only = origins.next()?.clone();
    origins.next().is_none().then_some(only)
}

fn iterator_bool_argument(arguments: &[TyArg], index: usize) -> Option<bool> {
    if let Some(TyArg::Val(crate::ct::CtValue::Bool(value))) = arguments.get(index) {
        return Some(*value);
    }
    let mut values = arguments.iter().filter_map(|argument| match argument {
        TyArg::Val(crate::ct::CtValue::Bool(value)) => Some(*value),
        TyArg::Ty(_) | TyArg::Origin(_) | TyArg::Val(_) => None,
    });
    let only = values.next()?;
    values.next().is_none().then_some(only)
}

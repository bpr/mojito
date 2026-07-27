//! Iterator-protocol selection for `for`/comprehension iterables and the
//! `for ref` bridge. Extracted from `checker.rs`; see `docs/symbol-map.md`.

use super::*;

impl Checker {
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
        owned: bool,
    ) -> Result<(Ty, crate::checked::IterationProtocol), TypeError> {
        use crate::checked::{IterationMode, IterationProtocol};
        let mode = if owned {
            IterationMode::Owned
        } else {
            IterationMode::Borrowed
        };
        let builtin = |element| {
            (
                element,
                IterationProtocol {
                    mode,
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
                Ok((
                    Ty::Assoc {
                        base: Box::new(ty.clone()),
                        name: "Element".to_string(),
                    },
                    IterationProtocol {
                        mode,
                        borrowed_origin: None,
                        reference: None,
                        prepare: vec![crate::symbol::iterator_dispatch_symbol(match mode {
                            IterationMode::Borrowed => crate::ast::ArgConvention::Read,
                            IterationMode::Owned => crate::ast::ArgConvention::Var,
                        })],
                        has_next: Some("__iterator_dispatch.__len__".to_string()),
                        next: Some("__iterator_dispatch.__next__".to_string()),
                        exhaustion: None,
                    },
                ))
            }
            other => Err(TypeError::TypeMismatch {
                expected: if owned {
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
                    .map(|(ret, error)| (sig, ret, error))
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
                    .map(|(ret, error)| (sig, ret, error))
            })
            .collect::<Vec<_>>();
        let [(next_sig, element, next_error)] = applicable_next.as_slice() else {
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
                    borrowed_origin: None,
                    reference: None,
                    prepare: vec![prepare_symbol],
                    has_next: None,
                    next: Some(next_symbol),
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
                    .map(|(ret, _)| (sig, ret))
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
                next: Some(next_symbol),
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
    ) -> Option<(Ty, Option<Ty>)> {
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
        Some((
            instantiate(&signature.ret),
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

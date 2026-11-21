//! Built-in receiver methods: Pointer, uninit storage, struct
//! dunders, List, and field invocations.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

/// The members of the declared `Pointer` a checker rule answers ahead of
/// the declaration: `infer_pointer_method`'s arms, the operator and
/// subscript rules, the construction rules, and the static rules. A member
/// off this list resolves to the declared body.
pub(in crate::checker) const POINTER_RULE_MEMBERS: &[&str] = &[
    "__int__",
    "unsafe_offset",
    "unsafe_write",
    "write",
    "unsafe_write_move_from",
    "unsafe_take_pointee",
    "unsafe_deinit_pointee",
    "free",
    "unsafe_free",
    "__eq__",
    "__ne__",
    "__lt__",
    "__le__",
    "__gt__",
    "__ge__",
    "__add__",
    "__sub__",
    "__iadd__",
    "__isub__",
    "__getitem__",
    "__setitem__",
    "__init__",
    "__merge_with__",
    "alloc",
    "alloc_aligned",
    "unsafe_dangling",
];

/// Whether a checker rule answers the declared `Pointer`'s `member`
/// ([`POINTER_RULE_MEMBERS`]).
pub(in crate::checker) fn pointer_rule_member(member: &str) -> bool {
    POINTER_RULE_MEMBERS.contains(&member)
}

/// The initializer keywords of the declared `Pointer` a checker rule
/// answers ahead of the declaration: `to=`. A construction by any other
/// keyword selects among the declared initializers, the synthesized
/// `copy=` included.
pub(in crate::checker) const POINTER_RULE_INITIALIZERS: &[&str] = &["to"];

/// Whether a checker rule answers a `Pointer` construction by `kwargs`:
/// one with no keyword, or with only [`POINTER_RULE_INITIALIZERS`] ones.
pub(in crate::checker) fn pointer_rule_construction(kwargs: &[mojito_ast::ast::KwArg]) -> bool {
    kwargs
        .iter()
        .all(|kwarg| POINTER_RULE_INITIALIZERS.contains(&kwarg.name.as_str()))
}

impl Checker {
    /// Type a `Pointer[T]` instance method: the public `unsafe_*` operation
    /// vocabulary (offset, write, take/deinit pointee, free) plus the
    /// deprecated `free()` bridge. Indexed load/store remain ordinary public
    /// pointer subscript syntax.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::checker) fn infer_pointer_method(
        &self,
        span: &SourceSpan,
        object: &Expr,
        method: &str,
        elem: &Ty,
        origin: &mojito_types::origin::PointerOrigin,
        address_space: &mojito_types::types::AddressSpaceSlot,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        let pointer = |element: Ty, origin: mojito_types::origin::PointerOrigin| {
            Ty::pointer(element, origin, address_space.clone())
        };
        // `unsafe_write` accepts the keyword `copy=` overload; every other
        // pointer method is positional-only.
        if !kwargs.is_empty() && method != "unsafe_write" {
            reject_kwargs(kwargs)?;
        }
        if !param_args.is_empty() {
            return Err(TypeError::BadCall {
                func: format!("Pointer.{method}"),
                reason: "compile-time parameter arguments are not supported here".to_string(),
            });
        }
        match method {
            // `Intable.__int__()`: the address, upstream's
            // `pop.pointer_to_index`.
            "__int__" => {
                no_arguments(method, args)?;
                self.record_builtin_read_receiver(object);
                Ok(Ty::int())
            }
            "unsafe_offset" => {
                if origin.as_origin().is_some() && !origin.multi_element() {
                    return Err(TypeError::Unsupported(
                        "pointer arithmetic and comparison are not supported on an \
                         origin-bearing Pointer to a single place"
                            .to_string(),
                    ));
                }
                if args.len() != 1 {
                    return Err(TypeError::ArityMismatch {
                        name: "unsafe_offset".to_string(),
                        expected: 1,
                        got: args.len(),
                    });
                }
                let offset = self.infer(&args[0])?;
                if !self.coerces_by_origins(&offset, &Ty::int()) {
                    return Err(TypeError::TypeMismatch {
                        expected: "Int".to_string(),
                        found: offset.to_string(),
                        context: "argument to 'Pointer.unsafe_offset'".to_string(),
                    });
                }
                self.record_pointer_adjustment(
                    span.clone(),
                    mojito_checked::checked::SemanticAdjustment::PointerOffset {
                        materialized: None,
                    },
                );
                Ok(pointer(elem.clone(), origin.clone()))
            }
            // `write` overwrites a pointee whose type has no destructor, so
            // it is `unsafe_write`'s store where upstream's `where
            // IsTriviallyDeinitable[U]` holds.
            "unsafe_write" | "write" => {
                self.check_pointer_write(object, origin)?;
                if method == "write" {
                    self.check_pointer_overwrite(elem)?;
                }
                let (value, copy) = match (args, kwargs) {
                    ([value], []) => (value, false),
                    ([], [keyword]) if keyword.name == "copy" => (&keyword.value, true),
                    _ => {
                        return Err(TypeError::BadCall {
                            func: "Pointer.unsafe_write".to_string(),
                            reason: "expected one positional value or a single 'copy=' \
                                     keyword argument"
                                .to_string(),
                        });
                    }
                };
                if copy && !self.is_copyable(elem) {
                    return Err(TypeError::TraitNotSatisfied {
                        param: "T".to_string(),
                        ty: elem.to_string(),
                        trait_name: "Copyable".to_string(),
                        reason: self.trait_failure_reason(elem, "Copyable"),
                    });
                }
                if copy {
                    self.copyable_reference_result_reads
                        .borrow_mut()
                        .insert(value.source_span());
                }
                let vty = self.infer_with_expected(value, elem, true)?;
                if !self.record_implicit_conversion(value, &vty, elem)? {
                    return Err(TypeError::TypeMismatch {
                        expected: elem.to_string(),
                        found: vty.to_string(),
                        context: "value written through 'Pointer.unsafe_write'".to_string(),
                    });
                }
                let receiver = pointer(elem.clone(), origin.clone());
                let (name, slot, convention) = if copy {
                    ("copy", ArgSlot::Keyword(0), None)
                } else {
                    ("value", ArgSlot::Positional(0), Some(ArgConvention::Var))
                };
                let bound = [elem.clone()];
                self.check_argument_origin_exclusivity(
                    &ExclusivityCallee {
                        name: method,
                        parameter_names: &[name.to_string()],
                        declared: &bound,
                        bound: &bound,
                        nested_origins: NestedOrigins::AsDeclared,
                        collected: None,
                        initializer: false,
                    },
                    Some(&ExclusivityReceiver {
                        object,
                        convention: None,
                        declared: &receiver,
                        bound: &receiver,
                    }),
                    &[convention],
                    &[slot],
                    args,
                    kwargs,
                )?;
                // `copy=` binds upstream's `init_pointee_copy(value: T)` read
                // slot: a place is lent where it lies, so the write's copy
                // is the one copy. Its declared `T` is bound by no
                // register-passable trait, so the slot is by address.
                if copy {
                    self.record_call_conventions(
                        span,
                        &[slot],
                        &[convention],
                        &crate::checker::places::DeclaredSlots {
                            by_address: vec![true],
                            writes: vec![false],
                        },
                        Gathered::NONE,
                    );
                    self.record_argument_borrows(
                        &[slot],
                        &[convention],
                        Gathered::NONE,
                        args,
                        kwargs,
                        None,
                    );
                }
                self.record_at(span.clone()).adjustment = Some(Box::new(
                    mojito_checked::checked::SemanticAdjustment::PointerWrite {
                        element: elem.clone(),
                        copy,
                    },
                ));
                Ok(Ty::none())
            }
            "unsafe_write_move_from" => {
                self.infer_pointer_write_move_from(span, object, elem, origin, address_space, args)
            }
            "unsafe_take_pointee" | "unsafe_deinit_pointee" => {
                self.check_pointer_take_source(span, method, elem, origin)?;
                no_arguments(method, args)?;
                if method == "unsafe_deinit_pointee" && !self.is_deinitable(elem) {
                    return Err(TypeError::TraitNotSatisfied {
                        param: "T".to_string(),
                        ty: elem.to_string(),
                        trait_name: "Deinitable".to_string(),
                        reason: self.trait_failure_reason(elem, "Deinitable"),
                    });
                }
                let adjustment = if method == "unsafe_take_pointee" {
                    mojito_checked::checked::SemanticAdjustment::PointerStorageTake {
                        element: elem.clone(),
                    }
                } else {
                    mojito_checked::checked::SemanticAdjustment::PointerStorageDestroy {
                        element: elem.clone(),
                    }
                };
                self.record_at(span.clone()).adjustment = Some(Box::new(adjustment));
                Ok(if method == "unsafe_take_pointee" {
                    elem.clone()
                } else {
                    Ty::none()
                })
            }
            "free" | "unsafe_free" => {
                if origin.as_origin().is_some() {
                    return Err(TypeError::Unsupported(format!(
                        "{method}() is not supported on an origin-bearing Pointer; \
                         it does not own an allocation"
                    )));
                }
                no_arguments(method, args)?;
                self.record_builtin_read_receiver(object);
                Ok(Ty::none())
            }
            _ => Err(TypeError::NoSuchMethod {
                object_type: pointer(elem.clone(), origin.clone()).to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Type a compiler-private inline uninit-storage method
    /// (`__UninitStorage[T]`'s `unsafe_write`/`take`/`destroy`), reachable
    /// only from the bundled crossing module. `take` and `destroy` consume
    /// their receiver, so they require an explicit `^` transfer; `unsafe_write`
    /// deliberately performs no lifecycle check on (and no drop of) a
    /// previously written payload — it leaks by design, like upstream.
    pub(super) fn infer_uninit_storage_method(
        &self,
        span: &SourceSpan,
        object: &Expr,
        method: &str,
        element: &Ty,
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        if !self.bundled_stdlib_declaration {
            return Err(TypeError::Unsupported(format!(
                "'{}' is compiler-private storage; use MaybeUninit from std.memory",
                mojito_types::types::UNINIT_STORAGE_TYPE_NAME
            )));
        }
        let storage_display = || {
            format!(
                "{}[{element}]",
                mojito_types::types::UNINIT_STORAGE_TYPE_NAME
            )
        };
        match method {
            "unsafe_write" => {
                if args.len() != 1 {
                    return Err(TypeError::ArityMismatch {
                        name: method.to_string(),
                        expected: 1,
                        got: args.len(),
                    });
                }
                let found = self.infer_with_expected(&args[0], element, true)?;
                if !self.record_implicit_conversion(&args[0], &found, element)? {
                    return Err(TypeError::TypeMismatch {
                        expected: element.to_string(),
                        found: found.to_string(),
                        context: format!("argument to '{method}'"),
                    });
                }
                self.record_at(span.clone()).adjustment = Some(Box::new(
                    mojito_checked::checked::SemanticAdjustment::UninitStorageWrite {
                        element: element.clone(),
                    },
                ));
                Ok(Ty::none())
            }
            "take" | "destroy" => {
                if !args.is_empty() {
                    return Err(TypeError::ArityMismatch {
                        name: method.to_string(),
                        expected: 0,
                        got: args.len(),
                    });
                }
                if !matches!(object.kind, ExprKind::Transfer(_)) {
                    return Err(TypeError::Unsupported(format!(
                        "{method}() consumes its storage; transfer it explicitly (`storage^.{method}()`)"
                    )));
                }
                if method == "destroy" && !self.is_deinitable(element) {
                    return Err(TypeError::TraitNotSatisfied {
                        param: "T".to_string(),
                        ty: element.to_string(),
                        trait_name: "Deinitable".to_string(),
                        reason: self.trait_failure_reason(element, "Deinitable"),
                    });
                }
                let adjustment = if method == "take" {
                    mojito_checked::checked::SemanticAdjustment::UninitStorageTake {
                        element: element.clone(),
                    }
                } else {
                    mojito_checked::checked::SemanticAdjustment::UninitStorageDestroy {
                        element: element.clone(),
                    }
                };
                self.record_at(span.clone()).adjustment = Some(Box::new(adjustment));
                Ok(if method == "take" {
                    element.clone()
                } else {
                    Ty::none()
                })
            }
            _ => Err(TypeError::NoSuchMethod {
                object_type: storage_display(),
                method: method.to_string(),
            }),
        }
    }

    /// If `recv` is a `struct` defining dunder method `name`, type the implicit
    /// call `recv.name(args…)` — the operator / subscript / builtin dispatch that
    /// turns a user struct into a first-class value type. Checks arity and argument
    /// coercion and returns the (type-argument-substituted) result type. Returns
    /// `None` when `recv` isn't a struct or has no such method, so the caller falls
    /// back to its own operator/builtin error.
    pub(in crate::checker) fn struct_dunder(
        &self,
        recv: &Ty,
        name: &str,
        args: &[&Ty],
    ) -> Option<Result<Ty, TypeError>> {
        let (info, sig, targs) = self.struct_dunder_signature_for(recv, name, args)?;
        let params: Vec<Ty> = sig
            .params
            .iter()
            .map(|t| substitute_at(t, info, &targs))
            .collect();
        if params.len() != args.len() {
            return Some(Err(TypeError::ArityMismatch {
                name: name.to_string(),
                expected: params.len(),
                got: args.len(),
            }));
        }
        for (arg, expected) in args.iter().zip(&params) {
            if !self.coerces_by_origins(arg, expected) {
                return Some(Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: arg.to_string(),
                    context: format!("argument to '{name}'"),
                }));
            }
        }
        Some(Ok(substitute_at(&sig.ret, info, &targs)))
    }

    /// The dunder overload implicit dispatch selects for `args`: among the
    /// same-arity declarations, the first whose parameters accept the operand
    /// types (`StringSpan.__eq__(rhs: Self)` beside `__eq__(rhs: String)`), else
    /// the first same-arity declaration so the mismatch diagnostic names a
    /// real signature. A compiler-typed receiver answers from the struct
    /// it is an instance of ([`Self::nominal_struct`]).
    pub(in crate::checker) fn struct_dunder_signature_for<'a>(
        &'a self,
        recv: &'a Ty,
        name: &str,
        args: &[&Ty],
    ) -> Option<(&'a StructInfo, &'a MethodSig, std::borrow::Cow<'a, [TyArg]>)> {
        let (sname, targs) = self.nominal_struct(recv)?;
        let info = self.structs.get(sname)?;
        let candidates = info.methods.get(name)?;
        let same_arity = || {
            candidates
                .iter()
                .filter(|sig| sig.params.len() == args.len())
        };
        let accepts = |sig: &MethodSig| {
            sig.params
                .iter()
                .zip(args)
                .all(|(param, arg)| self.value_coerces(arg, &substitute_at(param, info, &targs)))
        };
        let sig = same_arity()
            .find(|sig| accepts(sig))
            .or_else(|| same_arity().next())?;
        Some((info, sig, targs))
    }

    /// Type a `List` method call. The **mutating** methods (`append`, `insert`,
    /// `remove`, `pop`, `clear`, `reverse`, `extend`) require a plain variable
    /// receiver (so they can mutate its binding in place); the **query** methods
    /// (`count`, `index`) work on any list. `remove`/`count`/`index` require an
    /// equatable element type.
    pub(in crate::checker) fn infer_list_method(
        &self,
        object: &Expr,
        method: &str,
        elem: &Ty,
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        let no_such = || TypeError::NoSuchMethod {
            object_type: list_type(elem.clone()).to_string(),
            method: method.to_string(),
        };
        let mutating = matches!(
            method,
            "append" | "insert" | "remove" | "pop" | "clear" | "reverse" | "extend"
        );
        // A mutating method mutates its receiver, so the receiver must be a
        // writable place (a variable or a field/index chain rooted at one) —
        // not a temporary. Reading `check_place` validates exactly that.
        if mutating && self.check_place(object).is_err() {
            return Err(TypeError::MutationRequiresVariable(method.to_string()));
        }
        // `remove`/`count`/`index` compare elements, so require an equatable type.
        if matches!(method, "remove" | "count" | "index") && !self.is_list_equatable(elem) {
            return Err(TypeError::TypeMismatch {
                expected: "an equatable element type".to_string(),
                found: elem.to_string(),
                context: format!("'{method}'"),
            });
        }
        // Require the argument at position `i` to coerce to the element type.
        let expect_elem = |tys: &[Ty], i: usize| -> Result<(), TypeError> {
            if self.coerces_by_origins(&tys[i], elem) {
                Ok(())
            } else {
                Err(TypeError::TypeMismatch {
                    expected: elem.to_string(),
                    found: tys[i].to_string(),
                    context: format!("argument to '{method}'"),
                })
            }
        };
        match method {
            "append" => {
                let tys = self.builtin_args("append", 1, args)?;
                expect_elem(&tys, 0)?;
                Ok(Ty::none())
            }
            "insert" => {
                let tys = self.builtin_args("insert", 2, args)?;
                if !self.coerces_by_origins(&tys[0], &Ty::int()) {
                    return Err(TypeError::TypeMismatch {
                        expected: "Int".to_string(),
                        found: tys[0].to_string(),
                        context: "insert index".to_string(),
                    });
                }
                expect_elem(&tys, 1)?;
                Ok(Ty::none())
            }
            "remove" => {
                let tys = self.builtin_args("remove", 1, args)?;
                expect_elem(&tys, 0)?;
                Ok(Ty::none())
            }
            "pop" => {
                // `pop()` (last) or `pop(i)` — an optional `Int` index.
                if args.len() > 1 {
                    return Err(TypeError::ArityMismatch {
                        name: "pop".into(),
                        expected: 1,
                        got: args.len(),
                    });
                }
                if let Some(a) = args.first() {
                    let ity = self.infer(a)?;
                    if !self.coerces_by_origins(&ity, &Ty::int()) {
                        return Err(TypeError::TypeMismatch {
                            expected: "Int".to_string(),
                            found: ity.to_string(),
                            context: "pop index".to_string(),
                        });
                    }
                }
                Ok(elem.clone())
            }
            "clear" | "reverse" => {
                self.builtin_args(method, 0, args)?;
                Ok(Ty::none())
            }
            "extend" => {
                let tys = self.builtin_args("extend", 1, args)?;
                let expected = list_type(elem.clone());
                if tys[0] != expected {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: tys[0].to_string(),
                        context: "argument to 'extend'".to_string(),
                    });
                }
                Ok(Ty::none())
            }
            "count" | "index" => {
                let tys = self.builtin_args(method, 1, args)?;
                expect_elem(&tys, 0)?;
                Ok(Ty::int())
            }
            _ => Err(no_such()),
        }
    }

    /// Type a call through a callable-typed struct field: validate the
    /// arguments against the stored contract, replay any value-carried
    /// transfer effects, and mark the expression for indirect lowering.
    pub(super) fn infer_field_invocation(
        &self,
        span: SourceSpan,
        _object: &Expr,
        callable: &Ty,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        let (ret, _, error, ..) = self.infer_callable_ty(
            &span,
            "<callable>",
            callable.clone(),
            (&[], None),
            args,
            kwargs,
        )?;
        self.record_call_environment_effects(span.clone(), callable, &[], &[], args, kwargs)?;
        let carried = contract_transfer_effects(callable);
        if !carried.is_empty() {
            self.replay_transfer_effects(carried, None, args, &span, &TySubst::new())?;
        }
        if let Some(target) = self.indirect_callable_target(callable) {
            self.record_at(span.clone()).overload_target = Some(target);
        }
        self.record_at(span.clone()).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::FieldInvocation {
                callable: callable.clone(),
            },
        ));
        if let Some(error) = error.filter(|ty| *ty != TyKind::Never) {
            self.require_call_error(span, "call through a stored callable field", error)?;
        }
        Ok(ret)
    }

    /// Type `object.unsafe_write_move_from(args)` on a `Pointer[elem]` of
    /// `origin`: upstream's `__get_address_as_uninit_lvalue(self) =
    /// __get_address_as_owned_value(src)`, the source pointee taken and
    /// moved, with one move initializer, into the receiver's slot.
    fn infer_pointer_write_move_from(
        &self,
        span: &SourceSpan,
        object: &Expr,
        elem: &Ty,
        origin: &mojito_types::origin::PointerOrigin,
        address_space: &mojito_types::types::AddressSpaceSlot,
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        self.check_pointer_write(object, origin)?;
        let [source] = args else {
            return Err(TypeError::ArityMismatch {
                name: "unsafe_write_move_from".to_string(),
                expected: 1,
                got: args.len(),
            });
        };
        let receiver = Ty::pointer(elem.clone(), origin.clone(), address_space.clone());
        let found = self.infer_with_expected(source, &receiver, true)?;
        let Some((source_elem, source_origin)) = found.as_pointer() else {
            return Err(pointer_move_source_mismatch(elem, &found));
        };
        if &source_elem != elem || found.kind().pointer_address_space() != Some(address_space) {
            return Err(pointer_move_source_mismatch(elem, &found));
        }
        self.check_pointer_take_source(span, "unsafe_write_move_from", elem, &source_origin)?;
        if !self.is_movable(elem) {
            return Err(TypeError::TraitNotSatisfied {
                param: "T".to_string(),
                ty: elem.to_string(),
                trait_name: "Movable".to_string(),
                reason: self.trait_failure_reason(elem, "Movable"),
            });
        }
        self.record_at(span.clone()).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::PointerWriteMoveFrom {
                element: elem.clone(),
            },
        ));
        Ok(Ty::none())
    }

    /// Admit `method` taking the pointee `elem` out through a pointer of
    /// `origin`. Upstream requires only a mutable pointer. An origin-bearing
    /// pointee still belongs to its checked storage, which would destroy it
    /// again, so a tracked origin is accepted only for a trivially
    /// destructible element, or in the bundled modules
    /// `is_bundled_pointer_take_source` lists, which end that storage
    /// without destroying it (R2029).
    fn check_pointer_take_source(
        &self,
        span: &SourceSpan,
        method: &str,
        elem: &Ty,
        origin: &mojito_types::origin::PointerOrigin,
    ) -> Result<(), TypeError> {
        let untracked = matches!(
            origin,
            mojito_types::origin::PointerOrigin::Untracked { mutable: true }
        ) || super::super::overload_support::is_bundled_pointer_take_source(
            span.source.as_deref(),
        );
        if origin.statically_mutable() == Some(false) {
            return Err(TypeError::Unsupported(format!(
                "{method}() requires a Pointer with a mutable origin"
            )));
        }
        if !untracked && !self.is_trivially(mojito_types::types::TrivialLifecycle::Deinitable, elem)
        {
            return Err(TypeError::Unsupported(format!(
                "{method}() through an origin-bearing Pointer requires a trivially \
                 destructible element; '{elem}' is owned by its checked storage"
            )));
        }
        Ok(())
    }

    /// Require the pointee `elem` a `Pointer.write` overwrites to have no
    /// destructor, as upstream's `where IsTriviallyDeinitable[U]` does.
    fn check_pointer_overwrite(&self, elem: &Ty) -> Result<(), TypeError> {
        if self.is_trivially(mojito_types::types::TrivialLifecycle::Deinitable, elem) {
            return Ok(());
        }
        Err(TypeError::BadCall {
            func: "Pointer.write".to_string(),
            reason: format!("'{elem}' is not trivially deinitable; use 'unsafe_write'"),
        })
    }
}

/// Reject any argument to the nullary `Pointer` method `method`.
fn no_arguments(method: &str, args: &[Expr]) -> Result<(), TypeError> {
    if args.is_empty() {
        return Ok(());
    }
    Err(TypeError::ArityMismatch {
        name: method.to_string(),
        expected: 0,
        got: args.len(),
    })
}

/// The mismatch for a `Pointer.unsafe_write_move_from` source of type
/// `found` where a pointer to `elem` is required.
fn pointer_move_source_mismatch(elem: &Ty, found: &Ty) -> TypeError {
    TypeError::TypeMismatch {
        expected: format!("Pointer[{elem}, _]"),
        found: found.to_string(),
        context: "source of 'Pointer.unsafe_write_move_from'".to_string(),
    }
}

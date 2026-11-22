//! Built-in receiver methods: Pointer, uninit storage, struct
//! dunders, List, and field invocations.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

/// The members of the declared `Pointer` a checker rule answers ahead of
/// the declaration: `infer_pointer_method`'s arms, the construction
/// rules, and the static rules. A member off this list resolves to the
/// declared body: the subscript syntax (`p[]`, `p[i]`,
/// `p[unsafe_offset=i]`) and an explicit `p.__getitem__(…)` call both run
/// a declared getter.
pub(in crate::checker) const POINTER_RULE_MEMBERS: &[&str] = &[
    "write",
    "free",
    "unsafe_free",
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

impl Checker {
    /// Type the `Pointer[T]` instance members a checker rule answers:
    /// `write`, upstream's overwrite of a trivially destructible pointee,
    /// and the `free()`/`unsafe_free()` deallocation. Indexed load/store
    /// remain ordinary public pointer subscript syntax; every other memory
    /// member is a body of the declared struct.
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
        reject_kwargs(kwargs)?;
        if !param_args.is_empty() {
            return Err(TypeError::BadCall {
                func: format!("Pointer.{method}"),
                reason: "compile-time parameter arguments are not supported here".to_string(),
            });
        }
        match method {
            // `write` overwrites a pointee whose type has no destructor:
            // upstream's `where IsTriviallyDeinitable[U]` moved store.
            "write" => {
                self.check_pointer_write(object, origin)?;
                self.check_pointer_overwrite(elem)?;
                let [value] = args else {
                    return Err(TypeError::BadCall {
                        func: "Pointer.write".to_string(),
                        reason: "expected one positional value".to_string(),
                    });
                };
                let vty = self.infer_with_expected(value, elem, true)?;
                if !self.record_implicit_conversion(value, &vty, elem)? {
                    return Err(TypeError::TypeMismatch {
                        expected: elem.to_string(),
                        found: vty.to_string(),
                        context: "value written through 'Pointer.write'".to_string(),
                    });
                }
                let receiver = pointer(elem.clone(), origin.clone());
                let bound = [elem.clone()];
                self.check_argument_origin_exclusivity(
                    &ExclusivityCallee {
                        name: method,
                        parameter_names: &["value".to_string()],
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
                    &[Some(ArgConvention::Var)],
                    &[ArgSlot::Positional(0)],
                    args,
                    kwargs,
                )?;
                self.record_at(span.clone()).adjustment = Some(Box::new(
                    mojito_checked::checked::SemanticAdjustment::PointerWrite {
                        element: elem.clone(),
                    },
                ));
                Ok(Ty::none())
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

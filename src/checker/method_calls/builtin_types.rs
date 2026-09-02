//! Built-in receiver methods: Pointer, uninit storage, struct
//! dunders, List, Tuple, and field invocations.

use super::*;

impl Checker {
    /// Type a `Pointer[T]` instance method: the public `unsafe_*` operation
    /// vocabulary (offset, write, take/deinit pointee, free) plus the
    /// deprecated `free()` bridge. Indexed load/store remain ordinary public
    /// pointer subscript syntax.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::checker) fn infer_pointer_method(
        &self,
        span: &SourceSpan,
        method: &str,
        elem: &Ty,
        origin: &crate::origin::PointerOrigin,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        // `unsafe_write` accepts the keyword `copy=` overload; every other
        // pointer method is positional-only.
        if !kwargs.is_empty() && method != "unsafe_write" {
            reject_kwargs(kwargs)?;
        }
        // `unsafe_origin_cast` takes its target origin as the sole compile-time
        // parameter argument; no other pointer method is parameterized.
        if !param_args.is_empty() && method != "unsafe_origin_cast" {
            return Err(TypeError::BadCall {
                func: format!("Pointer.{method}"),
                reason: "compile-time parameter arguments are not supported here".to_string(),
            });
        }
        match method {
            // Provenance rebind (current Mojo's `unsafe_origin_cast`
            // vocabulary): the runtime value is unchanged; only the checked
            // origin moves. The cast cannot upgrade a statically immutable
            // capability.
            "unsafe_origin_cast" => {
                let [target] = param_args else {
                    return Err(TypeError::BadCall {
                        func: "Pointer.unsafe_origin_cast".to_string(),
                        reason: "expected exactly one origin parameter argument".to_string(),
                    });
                };
                if !args.is_empty() {
                    return Err(TypeError::ArityMismatch {
                        name: "unsafe_origin_cast".to_string(),
                        expected: 0,
                        got: args.len(),
                    });
                }
                let target = self.pointer_origin_arg(target)?;
                // A parametric target mutability is not an upgrade: it
                // resolves from the receiver at each concrete site.
                if origin.statically_mutable() == Some(false)
                    && target.statically_mutable() == Some(true)
                {
                    return Err(TypeError::Unsupported(
                        "an origin cast cannot upgrade capability: the source Pointer \
                         origin is immutable"
                            .to_string(),
                    ));
                }
                self.operation_adjustments.borrow_mut().insert(
                    span.clone(),
                    crate::checked::SemanticAdjustment::PointerOriginCast {
                        origin: target.clone(),
                    },
                );
                Ok(Ty::Pointer {
                    element: Box::new(elem.clone()),
                    origin: target,
                })
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
                if !coerces(&offset, &Ty::Int) {
                    return Err(TypeError::TypeMismatch {
                        expected: "Int".to_string(),
                        found: offset.to_string(),
                        context: "argument to 'Pointer.unsafe_offset'".to_string(),
                    });
                }
                self.operation_adjustments.borrow_mut().insert(
                    span.clone(),
                    crate::checked::SemanticAdjustment::PointerOffset,
                );
                Ok(Ty::Pointer {
                    element: Box::new(elem.clone()),
                    origin: origin.clone(),
                })
            }
            "unsafe_write" => {
                self.check_pointer_write(origin)?;
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
                let vty = self.infer(value)?;
                if !coerces(&vty, elem) {
                    return Err(TypeError::TypeMismatch {
                        expected: elem.to_string(),
                        found: vty.to_string(),
                        context: "value written through 'Pointer.unsafe_write'".to_string(),
                    });
                }
                self.operation_adjustments.borrow_mut().insert(
                    span.clone(),
                    crate::checked::SemanticAdjustment::PointerWrite {
                        element: elem.clone(),
                        copy,
                    },
                );
                Ok(Ty::None)
            }
            "unsafe_take_pointee" | "unsafe_deinit_pointee" => {
                if !matches!(
                    origin,
                    crate::origin::PointerOrigin::Untracked { mutable: true }
                ) {
                    return Err(TypeError::Unsupported(format!(
                        "{method}() requires an allocation-owning Pointer with a \
                         mutable untracked origin; an origin-bearing Pointer's \
                         pointee is owned by its checked storage"
                    )));
                }
                if !args.is_empty() {
                    return Err(TypeError::ArityMismatch {
                        name: method.to_string(),
                        expected: 0,
                        got: args.len(),
                    });
                }
                if method == "unsafe_deinit_pointee" && !self.is_deinitable(elem) {
                    return Err(TypeError::TraitNotSatisfied {
                        param: "T".to_string(),
                        ty: elem.to_string(),
                        trait_name: "Deinitable".to_string(),
                        reason: self.trait_failure_reason(elem, "Deinitable"),
                    });
                }
                let adjustment = if method == "unsafe_take_pointee" {
                    crate::checked::SemanticAdjustment::PointerStorageTake {
                        element: elem.clone(),
                    }
                } else {
                    crate::checked::SemanticAdjustment::PointerStorageDestroy {
                        element: elem.clone(),
                    }
                };
                self.operation_adjustments
                    .borrow_mut()
                    .insert(span.clone(), adjustment);
                Ok(if method == "unsafe_take_pointee" {
                    elem.clone()
                } else {
                    Ty::None
                })
            }
            "free" | "unsafe_free" => {
                if origin.as_origin().is_some() {
                    return Err(TypeError::Unsupported(format!(
                        "{method}() is not supported on an origin-bearing Pointer; \
                         it does not own an allocation"
                    )));
                }
                if !args.is_empty() {
                    return Err(TypeError::ArityMismatch {
                        name: method.to_string(),
                        expected: 0,
                        got: args.len(),
                    });
                }
                Ok(Ty::None)
            }
            _ => Err(TypeError::NoSuchMethod {
                object_type: Ty::Pointer {
                    element: Box::new(elem.clone()),
                    origin: origin.clone(),
                }
                .to_string(),
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
                crate::types::UNINIT_STORAGE_TYPE_NAME
            )));
        }
        let storage_display = || format!("{}[{element}]", crate::types::UNINIT_STORAGE_TYPE_NAME);
        match method {
            "unsafe_write" => {
                if args.len() != 1 {
                    return Err(TypeError::ArityMismatch {
                        name: method.to_string(),
                        expected: 1,
                        got: args.len(),
                    });
                }
                let found = self.infer(&args[0])?;
                if !coerces(&found, element) {
                    return Err(TypeError::TypeMismatch {
                        expected: element.to_string(),
                        found: found.to_string(),
                        context: format!("argument to '{method}'"),
                    });
                }
                self.operation_adjustments.borrow_mut().insert(
                    span.clone(),
                    crate::checked::SemanticAdjustment::UninitStorageWrite {
                        element: element.clone(),
                    },
                );
                Ok(Ty::None)
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
                    crate::checked::SemanticAdjustment::UninitStorageTake {
                        element: element.clone(),
                    }
                } else {
                    crate::checked::SemanticAdjustment::UninitStorageDestroy {
                        element: element.clone(),
                    }
                };
                self.operation_adjustments
                    .borrow_mut()
                    .insert(span.clone(), adjustment);
                Ok(if method == "take" {
                    element.clone()
                } else {
                    Ty::None
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
        let (info, sig, targs) = self.struct_dunder_signature(recv, name, args.len())?;
        let params: Vec<Ty> = sig
            .params
            .iter()
            .map(|t| substitute_at(t, &info.decls, targs))
            .collect();
        if params.len() != args.len() {
            return Some(Err(TypeError::ArityMismatch {
                name: name.to_string(),
                expected: params.len(),
                got: args.len(),
            }));
        }
        for (arg, expected) in args.iter().zip(&params) {
            if !coerces(arg, expected) {
                return Some(Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: arg.to_string(),
                    context: format!("argument to '{name}'"),
                }));
            }
        }
        Some(Ok(substitute_at(&sig.ret, &info.decls, targs)))
    }

    /// Resolve the declaration selected by implicit dunder dispatch. Callers
    /// that need convention semantics must inspect this signature before using
    /// the type-only `struct_dunder` result.
    pub(in crate::checker) fn struct_dunder_signature<'a>(
        &'a self,
        recv: &'a Ty,
        name: &str,
        arity: usize,
    ) -> Option<(&'a StructInfo, &'a MethodSig, &'a [TyArg])> {
        let Ty::Struct(sname, targs) = recv else {
            return None;
        };
        let info = self.structs.get(sname)?;
        let sig = info
            .methods
            .get(name)?
            .iter()
            .find(|sig| sig.params.len() == arity)?;
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
        if matches!(method, "remove" | "count" | "index") && !is_list_equatable(elem) {
            return Err(TypeError::TypeMismatch {
                expected: "an equatable element type".to_string(),
                found: elem.to_string(),
                context: format!("'{}'", method),
            });
        }
        // Require the argument at position `i` to coerce to the element type.
        let expect_elem = |tys: &[Ty], i: usize| -> Result<(), TypeError> {
            if coerces(&tys[i], elem) {
                Ok(())
            } else {
                Err(TypeError::TypeMismatch {
                    expected: elem.to_string(),
                    found: tys[i].to_string(),
                    context: format!("argument to '{}'", method),
                })
            }
        };
        match method {
            "append" => {
                let tys = self.builtin_args("append", 1, args)?;
                expect_elem(&tys, 0)?;
                Ok(Ty::None)
            }
            "insert" => {
                let tys = self.builtin_args("insert", 2, args)?;
                if !coerces(&tys[0], &Ty::Int) {
                    return Err(TypeError::TypeMismatch {
                        expected: "Int".to_string(),
                        found: tys[0].to_string(),
                        context: "insert index".to_string(),
                    });
                }
                expect_elem(&tys, 1)?;
                Ok(Ty::None)
            }
            "remove" => {
                let tys = self.builtin_args("remove", 1, args)?;
                expect_elem(&tys, 0)?;
                Ok(Ty::None)
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
                    if !coerces(&ity, &Ty::Int) {
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
                Ok(Ty::None)
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
                Ok(Ty::None)
            }
            "count" | "index" => {
                let tys = self.builtin_args(method, 1, args)?;
                expect_elem(&tys, 0)?;
                Ok(Ty::Int)
            }
            _ => Err(no_such()),
        }
    }

    /// Type the value-producing Tuple helpers in the current builtin surface.
    pub(in crate::checker) fn infer_tuple_method(
        &self,
        span: &SourceSpan,
        object: &Expr,
        method: &str,
        elements: &[Ty],
        call: MethodCallArguments<'_>,
    ) -> Result<Ty, TypeError> {
        let MethodCallArguments {
            param_args,
            args,
            parameterized_syntax,
            ..
        } = call;
        let receiver_implicitly_copyable = elements
            .iter()
            .all(|element| self.is_implicitly_copyable(element));
        match method {
            "reverse" => {
                if !param_args.is_empty() {
                    return Err(TypeError::WrongTypeArgCount {
                        name: "Tuple.reverse".to_string(),
                        expected: 0,
                        got: param_args.len(),
                    });
                }
                self.builtin_args("reverse", 0, args)?;
                if is_place_expr(object) && !receiver_implicitly_copyable {
                    return Err(TypeError::NonCopyable {
                        ty: nominal_tuple_type(elements.to_vec()).to_string(),
                        context:
                            "consuming receiver of method 'reverse' must be transferred with '^'"
                                .to_string(),
                    });
                }
                Ok(nominal_tuple_type(elements.iter().rev().cloned().collect()))
            }
            "concat" => {
                if !param_args.is_empty() {
                    return Err(TypeError::WrongTypeArgCount {
                        name: "Tuple.concat".to_string(),
                        expected: 0,
                        got: param_args.len(),
                    });
                }
                let tys = self.builtin_args("concat", 1, args)?;
                let Some(other) = tuple_elements(&tys[0]) else {
                    return Err(TypeError::TypeMismatch {
                        expected: "a Tuple".to_string(),
                        found: tys[0].to_string(),
                        context: "argument to 'concat'".to_string(),
                    });
                };
                if is_place_expr(object) && !receiver_implicitly_copyable {
                    return Err(TypeError::NonCopyable {
                        ty: nominal_tuple_type(elements.to_vec()).to_string(),
                        context:
                            "consuming receiver of method 'concat' must be transferred with '^'"
                                .to_string(),
                    });
                }
                if is_place_expr(&args[0])
                    && !other
                        .iter()
                        .all(|element| self.is_implicitly_copyable(element))
                {
                    return Err(TypeError::NonCopyable {
                        ty: tys[0].to_string(),
                        context:
                            "deinit argument 1 to method 'concat' must be transferred with '^'"
                                .to_string(),
                    });
                }
                let mut result = elements.to_vec();
                result.extend(other.into_iter().cloned());
                Ok(nominal_tuple_type(result))
            }
            "consume_elements" | "deinit_with" => {
                if !args.is_empty() {
                    return Err(TypeError::ArityMismatch {
                        name: format!("Tuple.{method}"),
                        expected: 0,
                        got: args.len(),
                    });
                }
                if is_place_expr(object) && !receiver_implicitly_copyable {
                    return Err(TypeError::NonCopyable {
                        ty: nominal_tuple_type(elements.to_vec()).to_string(),
                        context: format!(
                            "consuming receiver of method '{method}' must be transferred with '^'"
                        ),
                    });
                }
                let index_decl = ParamDecl::Value {
                    name: "index".to_string(),
                    ty: Box::new(Ty::Int),
                    default: None,
                    callable_default: None,
                    infer_only: false,
                    variadic: false,
                    constraints: Vec::new(),
                };
                let handler = Ty::GenericFunc {
                    environment: crate::origin::CallableEnvironment::Capturing(
                        crate::origin::CaptureOriginSet::empty(),
                    ),
                    decls: vec![index_decl],
                    params: vec![Ty::Dependent(DependentType::Indexed {
                        elements: elements.to_vec(),
                        index: CtExpr::Param("index".to_string()),
                    })],
                    names: vec!["element".to_string()],
                    ret: Box::new(Ty::None),
                    required: vec![true],
                    variadic: None,
                    kw_variadic: None,
                    positional_only: None,
                    keyword_only: None,
                    raises: false,
                    error: None,
                    conventions: vec![Some(ArgConvention::Var)],
                    ref_params: Box::new(vec![None]),
                    ref_return: None,
                    transfers: Default::default(),
                };
                let method_decls = vec![ParamDecl::Value {
                    name: "elt_handler".to_string(),
                    ty: Box::new(handler),
                    default: None,
                    callable_default: None,
                    infer_only: false,
                    variadic: false,
                    constraints: Vec::new(),
                }];
                self.resolve_use_params(
                    &format!("Tuple.{method}"),
                    &method_decls,
                    param_args,
                    &[],
                    &[],
                )?;
                if parameterized_syntax {
                    self.operation_adjustments.borrow_mut().insert(
                        span.clone(),
                        crate::checked::SemanticAdjustment::ParameterizedMethodCall {
                            param_decls: method_decls,
                        },
                    );
                }
                Ok(Ty::None)
            }
            _ => Err(TypeError::NoSuchMethod {
                object_type: nominal_tuple_type(elements.to_vec()).to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Type a call through a callable-typed struct field: validate the
    /// arguments against the stored contract, replay any value-carried
    /// transfer effects, and mark the expression for indirect lowering.
    pub(super) fn infer_field_invocation(
        &self,
        span: SourceSpan,
        _object: &Expr,
        callable: Ty,
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        let (ret, _, error, _) =
            self.infer_callable_ty(&span, "<callable>", callable.clone(), &[], args, kwargs)?;
        self.record_call_environment_effects(span.clone(), &callable, &[], args, kwargs)?;
        let carried = contract_transfer_effects(&callable);
        if !carried.is_empty() {
            self.replay_transfer_effects(carried, None, args, &span)?;
        }
        if let Some(target) = self.indirect_callable_target(&callable) {
            self.overload_targets
                .borrow_mut()
                .insert(span.clone(), target);
        }
        self.operation_adjustments.borrow_mut().insert(
            span.clone(),
            crate::checked::SemanticAdjustment::FieldInvocation {
                callable: callable.clone(),
            },
        );
        if let Some(error) = error.filter(|ty| *ty != Ty::Never) {
            self.record_call_effect(span.clone(), error.clone());
            self.require_error("call through a stored callable field", error)?;
        }
        Ok(ret)
    }
}

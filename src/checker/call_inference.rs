//! Free-function and callable-value call inference: `infer_call` dispatch,
//! callable-type application, generic-call instantiation, and forwarded-kwargs
//! element typing. Extracted from `checker.rs`; see `docs/symbol-map.md`.

use super::*;

type InferredCall = (Ty, usize, Option<Ty>, HashMap<usize, bool>);

impl Checker {
    fn erased_origin_constraint_environment(
        signature: &CallableOriginSignature,
        bindings: &HashMap<usize, bool>,
    ) -> Vec<(String, TyArg)> {
        bindings
            .iter()
            .filter_map(|(index, value)| {
                signature
                    .source
                    .get(*index)
                    .map(|parameter| (parameter.name.clone(), TyArg::Val(CtValue::Bool(*value))))
            })
            .collect()
    }

    fn erased_origin_constraint_applies(
        &self,
        signature: &CallableOriginSignature,
        bindings: &HashMap<usize, bool>,
    ) -> bool {
        if signature.availability.is_empty() {
            return true;
        }
        let owned = Self::erased_origin_constraint_environment(signature, bindings);
        let environment = owned
            .iter()
            .map(|(name, value)| (name.as_str(), value))
            .collect::<HashMap<_, _>>();
        signature
            .availability
            .iter()
            .all(|constraint| self.eval_generic_constraint(constraint, &environment))
    }

    /// The retained diagnostic for a failed erased-origin availability check:
    /// the first failing clause's message, when it declared one.
    fn first_failing_erased_origin_message(
        &self,
        signature: &CallableOriginSignature,
        bindings: &HashMap<usize, bool>,
    ) -> Option<String> {
        let owned = Self::erased_origin_constraint_environment(signature, bindings);
        let environment = owned
            .iter()
            .map(|(name, value)| (name.as_str(), value))
            .collect::<HashMap<_, _>>();
        signature
            .availability
            .iter()
            .find(|constraint| !self.eval_generic_constraint(constraint, &environment))
            .and_then(|constraint| match constraint {
                GenericConstraint::WithMessage(_, message) => {
                    Some(format!("constraint failed: {message}"))
                }
                _ => None,
            })
    }

    fn validate_erased_origin_constraint(
        &self,
        name: &str,
        signature: &CallableOriginSignature,
        bindings: &HashMap<usize, bool>,
    ) -> Result<(), TypeError> {
        if signature.availability.is_empty() {
            return Ok(());
        }
        let owned = Self::erased_origin_constraint_environment(signature, bindings);
        let environment = owned
            .iter()
            .map(|(name, value)| (name.as_str(), value))
            .collect::<HashMap<_, _>>();
        for constraint in &signature.availability {
            self.validate_constraint_in_environment(name, constraint, &environment)?;
        }
        Ok(())
    }

    /// Record the conversion plumbing that makes a builtin string-producing
    /// call yield the nominal `String` struct: route the call itself to the
    /// `"String"` conversion builtin (a `ResolveCallable` adjustment) and
    /// wrap its buffered result through the `@implicit` literal constructor.
    /// Falls back to the compile-time string result when the literal
    /// constructor is unavailable (a replaced stdlib root).
    pub(super) fn retarget_string_result(
        &self,
        span: SourceSpan,
        name: &str,
    ) -> Result<Ty, TypeError> {
        let nominal = Ty::Struct(name.to_string(), Vec::new());
        let Some((target, _)) = self.implicit_conversion_target(&Ty::StringLiteral, &nominal)?
        else {
            return Ok(Ty::StringLiteral);
        };
        self.overload_targets
            .borrow_mut()
            .insert(span.clone(), "String".to_string());
        self.implicit_conversions.borrow_mut().insert(span, target);
        Ok(nominal)
    }

    /// The wrap-only sibling of [`Self::retarget_string_result`] for builtin
    /// string producers whose callee stays itself (`input`, `repr`,
    /// `.format`): record the literal-constructor wrap at `span` and report
    /// the nominal `String`, falling back to the compile-time string when the
    /// stdlib constructor is unavailable (an unlinked seam).
    pub(super) fn nominal_string_wrap(&self, span: SourceSpan) -> Result<Ty, TypeError> {
        let nominal = Ty::Struct(crate::symbol::STDLIB_STRING_STRUCT.to_string(), Vec::new());
        let Some((target, _)) = self.implicit_conversion_target(&Ty::StringLiteral, &nominal)?
        else {
            return Ok(Ty::StringLiteral);
        };
        self.implicit_conversions.borrow_mut().insert(span, target);
        Ok(nominal)
    }

    pub(super) fn infer_call(
        &self,
        span: SourceSpan,
        name: &str,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        if is_variant_name(name)
            && (name != "Variant" || self.structs.contains_key(name))
            && self.lookup(name).is_none()
        {
            return self.infer_variant_construction(span, param_args, args, kwargs);
        }
        let ty = match self.lookup(name) {
            Some(ty) => ty.clone(),
            // Built-ins and struct construction, resolved only when the name
            // isn't shadowed by a binding.
            None => match name {
                _ if self.structs.contains_key(name) => {
                    // `String(x)` on a non-literal argument stringifies through
                    // the builtin conversion and materializes the result as the
                    // nominal struct: the call routes to the `"String"` builtin
                    // (a `ResolveCallable` adjustment) and the recorded
                    // implicit literal-constructor wrap builds the struct from
                    // the buffered text. A single string-literal argument (or a
                    // keyword construction such as `String(copy: s)`) is the
                    // ordinary constructor path.
                    if crate::symbol::is_stdlib_string_struct(name)
                        && !args.is_empty()
                        && kwargs.is_empty()
                        && param_args.is_empty()
                        && !(args.len() == 1 && matches!(self.infer(&args[0])?, Ty::StringLiteral))
                    {
                        self.infer_stringify(args)?;
                        return self.retarget_string_result(span, name);
                    }
                    return self.infer_construction(span, name, param_args, args, kwargs);
                }
                // Tuple specializations are predeclared as one closed set before
                // their members are checked.  A generated transform may therefore
                // construct its reverse result before that result's full StructInfo
                // has been populated (the reciprocal reverse direction makes any
                // sequential declaration order impossible).  Its concrete element
                // arguments are enough to validate the compiler-owned constructor;
                // `public_tuple_type` also proves that they select this exact
                // predeclared symbol.  Ordinary source constructors retain
                // sequential visibility because this gate is enabled only while a
                // compiler-generated Tuple implementation is being checked.
                _ if self.allow_generated_tuple_forward_types
                    && self.declared_structs.contains(name)
                    && (name.starts_with("Tuple$") || name.contains("$Tuple$"))
                    && param_args.is_empty()
                    && kwargs.is_empty() =>
                {
                    let tuple = self.infer_tuple_construction(&[], args)?;
                    if matches!(&tuple, Ty::Struct(target, _) if target == name) {
                        // Preserve the predeclared implementation as an exact
                        // checked callee.  This is intentionally redundant with
                        // the synthetic source spelling: MIR consumes checked
                        // call identity and never has to infer that a nominal
                        // Tuple construction is not the unspecialized template.
                        self.overload_targets
                            .borrow_mut()
                            .insert(span, name.to_string());
                        return Ok(tuple);
                    }
                    return Err(TypeError::BadCall {
                        func: name.to_string(),
                        reason: "generated Tuple constructor arguments select a different specialization"
                            .to_string(),
                    });
                }
                "Pointer" | "UnsafePointer" if !kwargs.is_empty() => {
                    return self.infer_pointer_to(span, param_args, args, kwargs);
                }
                // Compiler-private inline uninit storage: `__UninitStorage[T]()`
                // (uninitialized) or `__UninitStorage[T](value^)` (initialized),
                // reachable only from the bundled crossing module.
                _ if name == crate::types::UNINIT_STORAGE_TYPE_NAME => {
                    reject_kwargs(kwargs)?;
                    return self.infer_uninit_storage_construction(span, param_args, args);
                }
                _ if !kwargs.is_empty() => {
                    return Err(TypeError::BadCall {
                        func: name.to_string(),
                        reason: "keyword arguments are not supported here".to_string(),
                    });
                }
                "print" => return self.infer_print(args),
                "size_of" => {
                    if param_args.len() != 1 {
                        return Err(TypeError::WrongTypeArgCount {
                            name: "size_of".to_string(),
                            expected: 1,
                            got: param_args.len(),
                        });
                    }
                    if !args.is_empty() {
                        return Err(TypeError::ArityMismatch {
                            name: "size_of".to_string(),
                            expected: 0,
                            got: args.len(),
                        });
                    }
                    let ty = self.type_param_argument(&param_args[0], "size_of")?;
                    self.operation_adjustments
                        .borrow_mut()
                        .insert(span, crate::checked::SemanticAdjustment::SizeOf { ty });
                    return Ok(Ty::Int);
                }
                // Compiler-private crossing for `std.os.abort`: an uncatchable
                // VM trap carrying a nominal String message. Typed as returning
                // None — the call never returns at runtime, and stdlib call
                // sites keep a normal control-flow path after it rather than
                // relying on bottom-type flow analysis.
                "_mojito_abort" => {
                    let tys = self.builtin_args("_mojito_abort", 1, args)?;
                    match &tys[0] {
                        // A literal message avoids an os-module import cycle
                        // for stdlib-internal trap sites; the VM reads either
                        // spelling.
                        Ty::StringLiteral => return Ok(Ty::None),
                        Ty::Struct(name, targs)
                            if targs.is_empty() && crate::symbol::is_stdlib_string_struct(name) =>
                        {
                            return Ok(Ty::None);
                        }
                        other => {
                            return Err(TypeError::TypeMismatch {
                                expected: "String".to_string(),
                                found: other.to_string(),
                                context: "argument to '_mojito_abort'".to_string(),
                            });
                        }
                    }
                }
                // Positional `String(x)` is the stringify intrinsic; the
                // zero-argument form is the nominal empty constructor
                // (2026-08 stabilization) and falls through to it.
                "String" if !args.is_empty() => return self.infer_stringify(args),
                "repr" => {
                    let tys = self.builtin_args("repr", 1, args)?;
                    if self.conforms_to(&tys[0], "Writable") {
                        self.call_place_uses
                            .borrow_mut()
                            .insert(args[0].source_span());
                        return self.nominal_string_wrap(span);
                    }
                    return Err(TypeError::TypeMismatch {
                        expected: "Writable".to_string(),
                        found: tys[0].to_string(),
                        context: "argument to 'repr'".to_string(),
                    });
                }
                "hash" => {
                    let tys = self.builtin_args("hash", 1, args)?;
                    if self.conforms_to(&tys[0], "Hashable") {
                        return Ok(Ty::UInt);
                    }
                    return Err(TypeError::TraitNotSatisfied {
                        param: "T".to_string(),
                        ty: tys[0].to_string(),
                        trait_name: "Hashable".to_string(),
                        reason: self.trait_failure_reason(&tys[0], "Hashable"),
                    });
                }
                "abs" => return self.infer_abs(args),
                "min" | "max" => return self.infer_min_max(name, args),
                "round" => return self.infer_round(args),
                "input" => return self.infer_input(span, args),
                "len" => return self.infer_len(args),
                "range" => return self.infer_range(args),
                "Slice" | "slice" => return self.infer_slice_construction(name, args),
                "Int" => return self.infer_conversion(Ty::Int, args),
                "UInt" => return self.infer_conversion(Ty::UInt, args),
                "Float64" => return self.infer_conversion(Ty::Float64, args),
                "Bool" => return self.infer_conversion(Ty::Bool, args),
                "divmod" => return self.infer_divmod(args),
                "SIMD" => return self.infer_simd_construction(param_args, args),
                "Scalar" => {
                    if param_args.len() != 1 {
                        return Err(TypeError::WrongTypeArgCount {
                            name: "Scalar".to_string(),
                            expected: 1,
                            got: param_args.len(),
                        });
                    }
                    let dtype = dtype_from_arg(&param_args[0])?;
                    self.check_simd_args(dtype, 1, args)?;
                    return Ok(simd_ty(dtype, 1));
                }
                "List" => return self.infer_list_construction(param_args, args),
                "Set" => {
                    let collection = self.set_type(param_args)?;
                    let element = set_element(&collection)
                        .expect("Set type helper returns a nominal Set")
                        .clone();
                    for argument in args {
                        let actual = self.infer(argument)?;
                        if !coerces(&actual, &element) {
                            return Err(TypeError::TypeMismatch {
                                expected: element.to_string(),
                                found: actual.to_string(),
                                context: "Set construction element".to_string(),
                            });
                        }
                        self.record_literal_materializations(argument, &actual, &element)?;
                        self.check_consuming(argument, &actual, "Set construction element")?;
                    }
                    return Ok(set_type(element));
                }
                "Dict" => {
                    if !args.is_empty() {
                        return Err(TypeError::ArityMismatch {
                            name: "Dict".to_string(),
                            expected: 0,
                            got: args.len(),
                        });
                    }
                    return self.dict_type(param_args);
                }
                "Tuple" => {
                    let tuple = self.infer_tuple_construction(param_args, args)?;
                    if let Ty::Struct(target, _) = &tuple
                        && target != crate::types::TUPLE_TYPE_NAME
                    {
                        self.overload_targets
                            .borrow_mut()
                            .insert(span, target.clone());
                    }
                    return Ok(tuple);
                }
                "Error" => return self.infer_error_construction(args),
                _ if Dtype::from_scalar_alias(name).is_some() => {
                    let dtype = Dtype::from_scalar_alias(name)
                        .expect("match guard established a scalar alias");
                    return self.infer_simd_alias_construction(dtype, param_args, args);
                }
                _ => return Err(TypeError::UndefinedVariable(name.to_string())),
            },
        };
        self.record_permitted_call_capture(name);
        if let Some(owner) = self.lookup_owner(name) {
            self.expression_bindings
                .borrow_mut()
                .insert(span.clone(), owner);
        }
        let origin_signatures = self.lookup_callable_origins(name).unwrap_or_default();
        if let Ty::Overload(candidates) = ty {
            let mut matches = Vec::new();
            let mut availability_failures = Vec::new();
            for (index, candidate) in candidates.iter().enumerate() {
                let saved_conversions = self.implicit_conversions.borrow().clone();
                let saved_conversion_borrows = self.conversion_source_borrows.borrow().clone();
                let saved_invalidations = self.interior_invalidations.borrow().clone();
                let saved_call_place_uses = self.call_place_uses.borrow().clone();
                let saved_borrowed_read_places = self.borrowed_read_call_places.borrow().clone();
                if let Ok((prepared, ordinary_param_args)) = self.prepare_callable_specialization(
                    name,
                    param_args,
                    candidate.clone(),
                    origin_signatures.get(index),
                ) {
                    match self.infer_callable_ty(
                        &span,
                        name,
                        prepared.clone(),
                        &ordinary_param_args,
                        args,
                        kwargs,
                    ) {
                        Ok((ret, score, error, bool_bindings)) => {
                            if let Some(target) = callable_lowered_name(name, candidate) {
                                match origin_signatures.get(index) {
                                    Some(signature)
                                        if !self.erased_origin_constraint_applies(
                                            signature,
                                            &bool_bindings,
                                        ) =>
                                    {
                                        availability_failures.push(
                                            self.first_failing_erased_origin_message(
                                                signature,
                                                &bool_bindings,
                                            ),
                                        );
                                    }
                                    _ => matches.push((ret, score, target, error)),
                                }
                            }
                        }
                        Err(TypeError::BadCall { reason, .. })
                            if reason.starts_with("constraint failed: ")
                                || reason.starts_with("generic constraint is not satisfied: ") =>
                        {
                            // Generic constraints are validated while solving type
                            // arguments, before the final coercion and alias checks.
                            // Probe the same candidate without those availability
                            // predicates so only an otherwise call-compatible shape
                            // can contribute a retained diagnostic.
                            let mut unconstrained = prepared;
                            if let Ty::GenericFunc { decls, .. } = &mut unconstrained {
                                for decl in decls {
                                    match decl {
                                        ParamDecl::Type { constraints, .. }
                                        | ParamDecl::Value { constraints, .. } => {
                                            constraints.clear();
                                        }
                                    }
                                }
                            }
                            if self
                                .infer_callable_ty(
                                    &span,
                                    name,
                                    unconstrained,
                                    &ordinary_param_args,
                                    args,
                                    kwargs,
                                )
                                .is_ok()
                            {
                                availability_failures.push(
                                    reason.starts_with("constraint failed: ").then_some(reason),
                                );
                            }
                        }
                        Err(_) => {}
                    }
                }
                *self.implicit_conversions.borrow_mut() = saved_conversions;
                *self.conversion_source_borrows.borrow_mut() = saved_conversion_borrows;
                *self.interior_invalidations.borrow_mut() = saved_invalidations;
                *self.call_place_uses.borrow_mut() = saved_call_place_uses;
                *self.borrowed_read_call_places.borrow_mut() = saved_borrowed_read_places;
            }
            return match select_callable_overload(matches) {
                Ok((ret, target, error)) => {
                    self.overload_targets
                        .borrow_mut()
                        .insert(span.clone(), target.clone());
                    if let Some((index, selected)) =
                        candidates.iter().enumerate().find(|(_, candidate)| {
                            callable_lowered_name(name, candidate).as_deref()
                                == Some(target.as_str())
                        })
                    {
                        let (prepared, ordinary_param_args) = self
                            .prepare_callable_specialization(
                                name,
                                param_args,
                                selected.clone(),
                                origin_signatures.get(index),
                            )?;
                        self.infer_callable_ty(
                            &span,
                            name,
                            prepared.clone(),
                            &ordinary_param_args,
                            args,
                            kwargs,
                        )?;
                        self.record_call_environment_effects(
                            span.clone(),
                            &prepared,
                            &ordinary_param_args,
                            args,
                            kwargs,
                        )?;
                        // Overloads share the bare-name effect entry (a
                        // conservative union); replay it and any call-through
                        // residues exactly like the single-callable path.
                        self.apply_transfer_effects(name, None, args, &span)?;
                        self.record_call_through(name, &prepared, args);
                        let callee_decls = match &prepared {
                            Ty::GenericFunc { decls, .. } => decls.as_slice(),
                            _ => &[],
                        };
                        self.apply_call_through_effects(
                            name,
                            callee_decls,
                            None,
                            param_args,
                            args,
                            &span,
                        )?;
                    }
                    if let Some(error) = error.filter(|ty| *ty != Ty::Never) {
                        self.record_call_effect(span.clone(), error.clone());
                        self.require_error(format!("call to raising function '{name}'"), error)?;
                    }
                    self.record_view_result_borrow(&span, &ret);
                    Ok(ret)
                }
                Err(OverloadSelect::NoMatch) => {
                    // The stdlib scalar `range` family: upstream's overloads
                    // are dtype-inferred with no explicit spelling, so a
                    // scalar argument can never match the Int defs. The
                    // stdlib-set guard (a candidate returning a range-family
                    // struct) keeps a user's shadowing `range` overloads on
                    // the ordinary diagnostic path.
                    if name == "range"
                        && param_args.is_empty()
                        && kwargs.is_empty()
                        && candidates.iter().any(|candidate| {
                            matches!(
                                candidate,
                                Ty::Func { ret, .. }
                                    if matches!(
                                        ret.as_ref(),
                                        Ty::Struct(target, _)
                                            if crate::types::SCALAR_RANGE_FAMILY
                                                .iter()
                                                .any(|family| target.contains(family))
                                    )
                            )
                        })
                        && let Some(ty) = self.infer_scalar_range(&span, args)?
                    {
                        return Ok(ty);
                    }
                    Err(TypeError::BadCall {
                        func: name.to_string(),
                        reason: match availability_failures.as_slice() {
                            [Some(reason)] => reason.clone(),
                            _ => "no overload matches the supplied arguments".to_string(),
                        },
                    })
                }
                Err(OverloadSelect::Ambiguous) => Err(TypeError::BadCall {
                    func: name.to_string(),
                    reason: "ambiguous overloaded call".to_string(),
                }),
            };
        }
        // `objs[0](3)` / `grid[i, j](x)`: value brackets over an indexable
        // runtime binding are a subscript, not compile-time parameter
        // application — current Mojo dispatches the element call. A struct
        // carrying its own callable contract keeps parameter application
        // (specialized callable values), and non-value bracket forms keep the
        // parenthesization hint in `infer_callable_ty`.
        if let Ty::Struct(struct_name, _) = &ty
            && !param_args.is_empty()
            && param_args
                .iter()
                .all(|argument| matches!(argument, crate::ast::ParamArg::Value(_)))
            && self.declared_callable_contract(&ty).is_none()
            && self
                .structs
                .get(struct_name)
                .is_some_and(|info| info.methods.contains_key("__getitem__"))
        {
            let indices: Vec<Expr> = param_args
                .iter()
                .filter_map(|argument| match argument {
                    crate::ast::ParamArg::Value(value) => Some(value.clone()),
                    _ => None,
                })
                .collect();
            let receiver = Expr {
                kind: ExprKind::Identifier(name.to_string()),
                span: span.span,
                source: span.source.clone(),
                syntax_id: crate::token::SyntaxId::fresh(),
            };
            return self.infer_element_call(span, &receiver, &ty, &indices, args, kwargs);
        }
        let (ty, ordinary_param_args) =
            self.prepare_callable_specialization(name, param_args, ty, origin_signatures.first())?;
        let indirect_target = match &ty {
            Ty::Struct(..) => self.indirect_callable_target(&ty),
            _ if callable_contract_ty(&ty).is_some()
                && self.binding_scope(name).is_some_and(|scope| scope > 0) =>
            {
                self.indirect_callable_target(&ty)
            }
            _ => None,
        };
        if indirect_target.is_some() && matches!(ty, Ty::GenericFunc { .. }) {
            let (contract, arguments) =
                self.instantiate_generic_callable_value(name, ty.clone(), &ordinary_param_args)?;
            self.operation_adjustments.borrow_mut().insert(
                span.clone(),
                crate::checked::SemanticAdjustment::InstantiatedCallableContract {
                    contract,
                    arguments,
                },
            );
        }
        let (ret, _, error, bool_bindings) =
            self.infer_callable_ty(&span, name, ty.clone(), &ordinary_param_args, args, kwargs)?;
        if let Some(signature) = origin_signatures.first() {
            self.validate_erased_origin_constraint(name, signature, &bool_bindings)?;
        }
        // Replay the callee's loan-transfer effects against the actuals.
        self.apply_transfer_effects(name, None, args, &span)?;
        // A call through one of the current body's own callable parameters
        // records a higher-order residue for this body's callers to resolve;
        // a call to a callee WITH such residues resolves them against the
        // concrete callables this call supplies.
        self.record_call_through(name, &ty, args);
        let callee_decls = match &ty {
            Ty::GenericFunc { decls, .. } => decls.as_slice(),
            _ => &[],
        };
        self.apply_call_through_effects(name, callee_decls, None, param_args, args, &span)?;
        // A function-typed VALUE carries its origin def's effects on the
        // type; replay those too (the name-keyed entry above covers direct
        // and nested calls by declaration name).
        let carried = contract_transfer_effects(&ty);
        if !carried.is_empty() {
            self.replay_transfer_effects(carried, None, args, &span)?;
        }
        // A callable struct value dispatches `Struct.__call__`; replay its
        // effects with the callee binding as the receiver actual.
        if let Ty::Struct(struct_name, _) = &ty {
            let callee_value = Expr {
                kind: ExprKind::Identifier(name.to_string()),
                span: span.span,
                source: span.source.clone(),
                syntax_id: crate::token::SyntaxId::fresh(),
            };
            self.apply_transfer_effects(
                &format!("{struct_name}.__call__"),
                Some(&callee_value),
                args,
                &span,
            )?;
        }
        self.record_call_environment_effects(
            span.clone(),
            &ty,
            &ordinary_param_args,
            args,
            kwargs,
        )?;
        if let Some(target) = indirect_target {
            self.overload_targets
                .borrow_mut()
                .insert(span.clone(), target);
        }
        self.record_view_result_borrow(&span, &ret);
        if let Some(error) = error.filter(|ty| *ty != Ty::Never) {
            self.record_call_effect(span, error.clone());
            self.require_error(format!("call to raising function '{name}'"), error)?;
        }
        Ok(ret)
    }

    /// A non-method call whose result is a ref-field struct (a borrowing
    /// view/iterator) lends its borrowed sources to the result, exactly as the
    /// method-receiver rule in `infer_method_call` does: record the
    /// view-result adjustment so lowering anchors the result and establishes
    /// caller-side loans. Reference results carry their own loan channel, and
    /// an already-recorded adjustment (captures, instantiated contracts) wins.
    fn record_view_result_borrow(&self, span: &SourceSpan, ret: &Ty) {
        if matches!(ret, Ty::Struct(..)) && self.type_contains_reference(ret) {
            self.operation_adjustments
                .borrow_mut()
                .entry(span.clone())
                .or_insert(crate::checked::SemanticAdjustment::BorrowViewResult);
        }
    }

    /// Type the bare element-call spelling — `objs[0](3)`, `a.b[i](x)`,
    /// `grid[i, j](x)` — as subscript-then-indirect-call, matching current
    /// Mojo. The brackets are a runtime subscript of `receiver`, an indexable
    /// value with no callable contract of its own. The selected `__getitem__`
    /// moves out of `selected_calls` into the `ElementInvocation` adjustment
    /// so node-level consumers (reference-result diversion, resolved-callable
    /// projection, raise attribution) see the element call, not the subscript
    /// read.
    pub(super) fn infer_element_call(
        &self,
        span: SourceSpan,
        receiver: &Expr,
        receiver_ty: &Ty,
        indices: &[Expr],
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        let label = match &receiver.kind {
            ExprKind::Identifier(name) => name.clone(),
            ExprKind::Member { field, .. } => field.clone(),
            _ => "<element>".to_string(),
        };
        for (position, index) in indices.iter().enumerate() {
            self.prepare_index_argument(receiver_ty, index, "__getitem__", position)?;
        }
        let result =
            self.infer_struct_getitem_call(span.clone(), receiver, indices, receiver_ty)?;
        let element_ty = match result {
            Ty::Ref(reference) => *reference.referent,
            value => value,
        };
        let getter = self
            .selected_calls
            .borrow_mut()
            .remove(&span)
            .ok_or_else(|| {
                TypeError::InvariantViolation(
                    "element-call subscript selection recorded no contract".to_string(),
                )
            })?;
        let Some(callable) = self.declared_callable_contract(&element_ty) else {
            return Err(TypeError::NotCallable {
                name: format!("{label}[…]"),
                ty: element_ty.to_string(),
            });
        };
        // The getter's dispatch entry must not survive as this node's resolved
        // callable: `CallIndirect` consults the element's `__call__` target.
        let target = self.indirect_callable_target(&element_ty);
        match &target {
            Some(target) => {
                self.overload_targets
                    .borrow_mut()
                    .insert(span.clone(), target.clone());
            }
            None => {
                self.overload_targets.borrow_mut().remove(&span);
            }
        }
        let (ret, _, error, _) =
            self.infer_callable_ty(&span, "<element>", callable.clone(), &[], args, kwargs)?;
        let carried = contract_transfer_effects(&callable);
        if !carried.is_empty() {
            self.replay_transfer_effects(carried, None, args, &span)?;
        }
        if let Ty::Struct(struct_name, _) = &element_ty {
            self.apply_transfer_effects(
                &format!("{struct_name}.__call__"),
                Some(receiver),
                args,
                &span,
            )?;
        }
        self.record_call_environment_effects(span.clone(), &callable, &[], args, kwargs)?;
        self.operation_adjustments.borrow_mut().insert(
            span.clone(),
            crate::checked::SemanticAdjustment::ElementInvocation(Box::new(
                crate::checked::CheckedElementInvocation {
                    getter,
                    callable,
                    target,
                    raises: error.clone(),
                },
            )),
        );
        if let Some(error) = error.filter(|ty| *ty != Ty::Never) {
            self.record_call_effect(span.clone(), error.clone());
            self.require_error("call through a raising callable element", error)?;
        }
        Ok(ret)
    }

    pub(super) fn infer_callable_ty(
        &self,
        span: &SourceSpan,
        name: &str,
        ty: Ty,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<InferredCall, TypeError> {
        let (
            params,
            names,
            ret,
            required,
            variadic,
            kw_variadic,
            positional_only,
            keyword_only,
            _raises,
            error,
            conventions,
            ref_params,
            ref_return,
        ) = match ty {
            Ty::Param {
                callable_bound: Some(bound),
                ..
            } => {
                return self.infer_callable_ty(span, name, *bound, param_args, args, kwargs);
            }
            Ty::Struct(struct_name, arguments) => {
                let actual = Ty::Struct(struct_name.clone(), arguments);
                let callable = self.declared_callable_contract(&actual).ok_or_else(|| {
                    // Value brackets over an indexable runtime value were
                    // re-dispatched as an element call before reaching here,
                    // so this shape carries type, named, or empty bracket
                    // arguments — neither a runtime subscript nor parameter
                    // application on a callable.
                    if !param_args.is_empty()
                        && self
                            .structs
                            .get(&struct_name)
                            .is_some_and(|info| info.methods.contains_key("__getitem__"))
                    {
                        return TypeError::Unsupported(format!(
                            "'{name}' is a runtime value of type '{struct_name}': \
                             '{name}[…](…)' dispatches a subscripted element call only \
                             for runtime index arguments, and a value takes no \
                             compile-time parameters"
                        ));
                    }
                    TypeError::NotCallable {
                        name: name.to_string(),
                        ty: struct_name.clone(),
                    }
                })?;
                return self.infer_callable_ty(span, name, callable, param_args, args, kwargs);
            }
            // A non-generic function takes no compile-time parameters.
            Ty::Func {
                params,
                names,
                ret,
                required,
                variadic,
                kw_variadic,
                positional_only,
                keyword_only,
                raises,
                error,
                conventions,
                ref_params,
                ref_return,
                ..
            } => {
                if !param_args.is_empty() {
                    return Err(TypeError::WrongTypeArgCount {
                        name: name.to_string(),
                        expected: 0,
                        got: param_args.len(),
                    });
                }
                (
                    params,
                    names,
                    ret,
                    required,
                    variadic,
                    kw_variadic,
                    positional_only,
                    keyword_only,
                    raises,
                    error,
                    conventions,
                    ref_params,
                    ref_return,
                )
            }
            // Bind ordinary arguments first, then infer or apply the generic
            // function's compile-time parameters from the occupied slots.
            generic @ Ty::GenericFunc { .. } => {
                return self.infer_generic_call(span, name, &generic, param_args, args, kwargs);
            }
            other => {
                return Err(TypeError::NotCallable {
                    name: name.to_string(),
                    ty: other.to_string(),
                });
            }
        };

        // Match positional then keyword arguments to the regular parameter slots
        // (extra positional args overflow into a `*args` parameter), then check
        // each supplied argument coerces to its parameter's type (an unfilled slot
        // uses the default, already type-checked at the definition site).
        let forwarded_element = self.forwarded_kwargs_element(name, kwargs)?;
        let kw_names: Vec<&str> = kwargs
            .iter()
            .filter(|argument| !argument.is_forwarded())
            .map(|argument| argument.name.as_str())
            .collect();
        let has_kw_collector = kw_variadic.is_some();
        let kw_collector = kw_variadic.map(|element| *element);
        if forwarded_element.is_some() && kw_collector.is_none() {
            return Err(TypeError::BadCall {
                func: name.to_string(),
                reason: "`**kwargs^` requires a callee with a `**kwargs` collector".to_string(),
            });
        }
        let matched = match_call_slots(
            &names,
            &required,
            positional_only,
            keyword_only,
            args.len(),
            &kw_names,
            CallVariadics {
                positional: variadic.is_some(),
                keyword: kw_collector.is_some(),
            },
        )
        .map_err(|e| e.into_type_error(name))?;
        let (slots, overflow, kw_overflow) = (
            matched.slots,
            matched.positional_overflow,
            matched.keyword_overflow,
        );
        let mut score = 0;
        for (i, slot) in slots.iter().enumerate() {
            let arg = match slot {
                ArgSlot::Positional(p) => &args[*p],
                ArgSlot::Keyword(k) => &kwargs[*k].value,
                ArgSlot::Default => continue,
            };
            let arg_ty = self.infer_with_expected(arg, &params[i], true)?;
            if !self.record_implicit_conversion(arg, &arg_ty, &params[i])? {
                if super::builtins::callable_mismatch_is_environment_only(&arg_ty, &params[i]) {
                    return Err(TypeError::Unsupported(format!(
                        "a capturing closure cannot bind to the unqualified 'def(...)' \
                         parameter '{}' of '{}' in current Mojo; the contract must spell \
                         'capturing[...]'",
                        names[i], name
                    )));
                }
                return Err(TypeError::TypeMismatch {
                    expected: params[i].to_string(),
                    found: arg_ty.to_string(),
                    context: format!("argument '{}' to '{}'", names[i], name),
                });
            }
            score += conversion_count(&arg_ty, &params[i]);
            // Only a `var`/`deinit` parameter *consumes* its argument (moving the
            // value in). `read` (the default), `mut`, and `ref` all **borrow** — no
            // copy — so passing a non-Copyable value to them is fine.
            if let Some(Some(convention @ (ArgConvention::Var | ArgConvention::Deinit))) =
                conventions.get(i)
            {
                let kind = if *convention == ArgConvention::Deinit {
                    super::traits::ConsumeKind::Deinit
                } else {
                    super::traits::ConsumeKind::Move
                };
                self.check_consuming_as(
                    arg,
                    &arg_ty,
                    &format!("argument '{}' to '{}'", names[i], name),
                    kind,
                )?;
            }
        }
        // Each overflow argument must coerce to the `*args` element type.
        if let Some(elem) = &variadic {
            for (pack_index, &p) in overflow.iter().enumerate() {
                let expected = match &**elem {
                    Ty::RuntimePack(elements) => {
                        elements
                            .get(pack_index)
                            .ok_or_else(|| TypeError::ArityMismatch {
                                name: name.to_string(),
                                expected: elements.len(),
                                got: overflow.len(),
                            })?
                    }
                    _ => elem,
                };
                let arg_ty = self.infer_with_expected(&args[p], expected, true)?;
                if !coerces(&arg_ty, expected) {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: arg_ty.to_string(),
                        context: format!("variadic argument to '{}'", name),
                    });
                }
                score += conversion_count(&arg_ty, expected);
            }
            if let Ty::RuntimePack(elements) = &**elem
                && elements.len() != overflow.len()
            {
                return Err(TypeError::ArityMismatch {
                    name: name.to_string(),
                    expected: elements.len(),
                    got: overflow.len(),
                });
            }
        }
        if let Some(elem) = kw_collector {
            for index in kw_overflow {
                let expression = &kwargs[index].value;
                let found = self.infer_with_expected(expression, &elem, true)?;
                if !self.record_implicit_conversion(expression, &found, &elem)? {
                    return Err(TypeError::TypeMismatch {
                        expected: elem.to_string(),
                        found: found.to_string(),
                        context: format!(
                            "keyword '{}' collected by '{}'",
                            kwargs[index].name, name
                        ),
                    });
                }
                self.check_consuming(
                    expression,
                    &found,
                    &format!("keyword '{}' collected by '{name}'", kwargs[index].name),
                )?;
                score += conversion_count(&found, &elem);
            }
            if let Some(found) = forwarded_element
                && found != elem
            {
                return Err(TypeError::TypeMismatch {
                    expected: format!("StringDict[{elem}]"),
                    found: format!("StringDict[{found}]"),
                    context: format!("forwarded keyword arguments to '{name}'"),
                });
            }
        }

        // Borrow check (mutable-XOR-shared), root-sensitive: within one call a
        // variable borrowed exclusively (`mut`/`ref`) or moved (`^`) may not be
        // borrowed again — mutably, shared, or moved.
        let (effective_conventions, return_ref, bool_bindings) = self
            .solve_call_origins_with_bool_bindings(
                &slots,
                &conventions,
                &ref_params,
                ref_return.as_deref(),
                args,
                kwargs,
            )?;
        let copied_reads = slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let expression = match slot {
                    ArgSlot::Positional(position) => &args[*position],
                    ArgSlot::Keyword(position) => &kwargs[*position].value,
                    ArgSlot::Default => return Ok(false),
                };
                let convention = effective_conventions.get(index).copied().flatten();
                Ok(
                    !matches!(convention, Some(ArgConvention::Mut | ArgConvention::Ref))
                        && self.is_copyable(&self.infer_with_expected(
                            expression,
                            &params[index],
                            true,
                        )?),
                )
            })
            .collect::<Result<Vec<_>, TypeError>>()?;
        check_call_aliasing(&slots, &effective_conventions, &copied_reads, args, kwargs)?;
        self.borrowed_read_call_places
            .borrow_mut()
            .extend(borrowable_read_arguments(
                &slots,
                &effective_conventions,
                args,
                kwargs,
                None,
            ));

        let result = return_ref
            .map(|mut reference| {
                reference.referent = ret.clone();
                Ty::Ref(reference)
            })
            .unwrap_or(*ret);
        Ok((
            result,
            overload_rank(score, variadic.is_some() || has_kw_collector, 0, false),
            error.map(|error| *error),
            bool_bindings,
        ))
    }

    /// Type a call to a generic function: solve its type parameters from the
    /// argument types, then check each argument coerces to the substituted
    /// parameter type and return the substituted result type.
    pub(super) fn infer_generic_call(
        &self,
        span: &SourceSpan,
        name: &str,
        generic: &Ty,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<InferredCall, TypeError> {
        let Ty::GenericFunc {
            decls,
            params,
            names,
            ret,
            required,
            variadic,
            kw_variadic,
            positional_only,
            keyword_only,
            raises: _,
            error,
            conventions,
            ref_params,
            ref_return,
            ..
        } = generic
        else {
            return Err(TypeError::InvariantViolation(format!(
                "generic call inference received non-generic callee '{name}'"
            )));
        };
        let forwarded_element = self.forwarded_kwargs_element(name, kwargs)?;
        if forwarded_element.is_some() && kw_variadic.is_none() {
            return Err(TypeError::BadCall {
                func: name.to_string(),
                reason: "`**kwargs^` requires a callee with a `**kwargs` collector".to_string(),
            });
        }
        let kw_names: Vec<&str> = kwargs
            .iter()
            .filter(|argument| !argument.is_forwarded())
            .map(|argument| argument.name.as_str())
            .collect();
        let matched = match_call_slots(
            names,
            required,
            *positional_only,
            *keyword_only,
            args.len(),
            &kw_names,
            CallVariadics {
                positional: variadic.is_some(),
                keyword: kw_variadic.is_some(),
            },
        )
        .map_err(|e| e.into_type_error(name))?;
        let (slots, overflow, kw_overflow) = (
            matched.slots,
            matched.positional_overflow,
            matched.keyword_overflow,
        );
        let mut use_params = Vec::new();
        let mut arg_tys = Vec::new();
        let mut arg_exprs = Vec::new();
        for (i, slot) in slots.iter().enumerate() {
            let arg = match slot {
                ArgSlot::Positional(p) => &args[*p],
                ArgSlot::Keyword(k) => &kwargs[*k].value,
                ArgSlot::Default => continue,
            };
            use_params.push(params[i].clone());
            arg_tys.push(self.infer(arg)?);
            arg_exprs.push(arg);
        }
        if let Some(elem) = variadic.as_deref() {
            for &p in &overflow {
                use_params.push(elem.clone());
                arg_tys.push(self.infer(&args[p])?);
                arg_exprs.push(&args[p]);
            }
        }
        let mut keyword_actuals = Vec::new();
        if let Some(element) = kw_variadic.as_deref() {
            for &index in &kw_overflow {
                let actual = self.infer(&kwargs[index].value)?;
                use_params.push(element.clone());
                arg_tys.push(actual.clone());
                keyword_actuals.push((index, actual));
            }
            if let Some(actual) = &forwarded_element {
                use_params.push(element.clone());
                arg_tys.push(actual.clone());
            }
        }
        let (subst, tyargs) =
            self.resolve_use_params(name, decls, param_args, &use_params, &arg_tys)?;
        let values = Self::value_argument_environment(decls, &tyargs);
        let resolve = |ty: &Ty| {
            let substituted = self.resolve_assoc_ty(&substitute(ty, &subst));
            self.resolve_dependent_ty(&substituted, &values)
        };
        let mut conversions = 0;
        for ((aty, pty), expression) in arg_tys.iter().zip(&use_params).zip(arg_exprs) {
            if matches!(pty, Ty::Param { name, .. } if name.starts_with('*')) {
                // Each pack element was checked independently against the pack's
                // bounds during inference; there is intentionally no single
                // substituted element type to coerce every argument into.
                continue;
            }
            let expected = resolve(pty)?;
            // A dependent generic parameter can resolve to a reference-valued
            // type only after explicit value arguments have been substituted
            // (for example `Ts[index]` in Tuple.consume_elements). Re-infer in
            // that resolved context so the actual is the stored handle rather
            // than the ordinary read-through referent.
            let contextual;
            let actual = if self.type_contains_reference(&expected) {
                contextual = self.infer_with_expected(expression, &expected, true)?;
                &contextual
            } else {
                aty
            };
            if !self.record_implicit_conversion(expression, actual, &expected)? {
                return Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: actual.to_string(),
                    context: format!("argument to '{}'", name),
                });
            }
            conversions += conversion_count(actual, &expected);
        }
        if let Some(element) = kw_variadic.as_deref() {
            let expected = resolve(element)?;
            for (index, actual) in keyword_actuals {
                let expression = &kwargs[index].value;
                if !self.record_implicit_conversion(expression, &actual, &expected)? {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: actual.to_string(),
                        context: format!(
                            "keyword '{}' collected by '{}'",
                            kwargs[index].name, name
                        ),
                    });
                }
                self.check_consuming(
                    expression,
                    &actual,
                    &format!("keyword '{}' collected by '{name}'", kwargs[index].name),
                )?;
                conversions += conversion_count(&actual, &expected);
            }
            if let Some(actual) = forwarded_element
                && actual != expected
            {
                return Err(TypeError::TypeMismatch {
                    expected: format!("StringDict[{expected}]"),
                    found: format!("StringDict[{actual}]"),
                    context: format!("forwarded keyword arguments to '{name}'"),
                });
            }
        }
        for (i, slot) in slots.iter().enumerate() {
            if let Some(Some(convention @ (ArgConvention::Var | ArgConvention::Deinit))) =
                conventions.get(i)
            {
                let arg = match slot {
                    ArgSlot::Positional(p) => &args[*p],
                    ArgSlot::Keyword(k) => &kwargs[*k].value,
                    ArgSlot::Default => continue,
                };
                let kind = if *convention == ArgConvention::Deinit {
                    super::traits::ConsumeKind::Deinit
                } else {
                    super::traits::ConsumeKind::Move
                };
                let expected = resolve(&params[i])?;
                let ty = self.infer_with_expected(arg, &expected, true)?;
                self.check_consuming_as(
                    arg,
                    &ty,
                    &format!("argument '{}' to '{}'", names[i], name),
                    kind,
                )?;
            }
        }
        let (effective_conventions, return_ref, bool_bindings) = self
            .solve_call_origins_with_bool_bindings(
                &slots,
                conventions,
                ref_params,
                ref_return.as_deref(),
                args,
                kwargs,
            )?;
        let copied_reads = slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let expression = match slot {
                    ArgSlot::Positional(position) => &args[*position],
                    ArgSlot::Keyword(position) => &kwargs[*position].value,
                    ArgSlot::Default => return Ok(false),
                };
                let convention = effective_conventions.get(index).copied().flatten();
                Ok(
                    !matches!(convention, Some(ArgConvention::Mut | ArgConvention::Ref))
                        && self.is_copyable(&self.infer(expression)?),
                )
            })
            .collect::<Result<Vec<_>, TypeError>>()?;
        check_call_aliasing(&slots, &effective_conventions, &copied_reads, args, kwargs)?;
        self.borrowed_read_call_places
            .borrow_mut()
            .extend(borrowable_read_arguments(
                &slots,
                &effective_conventions,
                args,
                kwargs,
                None,
            ));
        let referent = self.canonicalize_public_tuple_types(resolve(ret)?);
        let result = return_ref
            .map(|mut reference| {
                reference.referent = Box::new(referent.clone());
                Ty::Ref(reference)
            })
            .unwrap_or(referent);
        let error = error.as_ref().map(|error| resolve(error)).transpose()?;
        // Retain the resolved application for instantiation discovery.
        // Speculative overload attempts overwrite the same span; the selected
        // candidate's re-run writes last.
        self.generic_instantiations.borrow_mut().insert(
            span.clone(),
            crate::checked::GenericInstantiation {
                callee: name.to_string(),
                arguments: tyargs.clone(),
            },
        );
        Ok((
            result,
            overload_rank(
                conversions,
                variadic.is_some() || kw_variadic.is_some(),
                decls.len(),
                true,
            ),
            error,
            bool_bindings,
        ))
    }

    pub(super) fn forwarded_kwargs_element(
        &self,
        callee: &str,
        kwargs: &[crate::ast::KwArg],
    ) -> Result<Option<Ty>, TypeError> {
        let mut forwarded = kwargs.iter().filter(|argument| argument.is_forwarded());
        let Some(argument) = forwarded.next() else {
            return Ok(None);
        };
        if forwarded.next().is_some() {
            return Err(TypeError::BadCall {
                func: callee.to_string(),
                reason: "only one keyword dictionary can be forwarded".to_string(),
            });
        }
        if !matches!(&argument.value.kind, ExprKind::Transfer(_)) {
            return Err(TypeError::BadCall {
                func: callee.to_string(),
                reason: "keyword forwarding requires ownership transfer (`**kwargs^`)".to_string(),
            });
        }
        let found = self.infer(&argument.value)?;
        match found {
            Ty::Struct(name, args) if name == "StringDict" => match args.as_slice() {
                [TyArg::Ty(element)] => Ok(Some(element.clone())),
                _ => Err(TypeError::InvariantViolation(
                    "StringDict must carry one value type".to_string(),
                )),
            },
            other => Err(TypeError::TypeMismatch {
                expected: "StringDict[T]".to_string(),
                found: other.to_string(),
                context: format!("forwarded keyword arguments to '{callee}'"),
            }),
        }
    }
}

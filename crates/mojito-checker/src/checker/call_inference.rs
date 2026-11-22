//! Free-function and callable-value call inference: `infer_call` dispatch,
//! callable-type application, generic-call instantiation, and forwarded-kwargs
//! element typing. Extracted from `checker.rs`; see `docs/symbol-map.md`.

use super::constraints::LACKING_EVIDENCE;
use super::origins::CallValues;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_symbol::symbol::TypeKey;
use mojito_types::types::{FuncTy, GenericFuncTy};
use mojito_types::types::{POINTER_TYPE_NAME, TyKind};

/// A call typed against one callable: its result, rank, error type, the
/// erased-origin bindings, and how many arguments with a lane it binds to a
/// bare type parameter (`CallableOverloadMatch::simd_erasures`).
/// A typed call: its result, overload rank, raised error, bool origin
/// bindings, SIMD erasures, and what each of its bracket arguments bound to.
/// A checked call: its result, overload score, error type, the `Bool`
/// binders its origins bound, its SIMD erasures, the attributes its bracket
/// bound over the callee's declared list, and the callee with what the
/// bracket bound substituted into its signature.
type InferredCall = (
    Ty,
    usize,
    Option<Ty>,
    HashMap<usize, bool>,
    usize,
    DeclaredAttributes,
    Ty,
);

impl Checker {
    /// Reject a reference to the module function `name` as a value when
    /// it is `@unavailable`, as a call selecting it is.
    pub(super) fn reject_unavailable_reference(&self, name: &str) -> Result<(), TypeError> {
        if self.binding_scope(name) != Some(0) {
            return Ok(());
        }
        match self.lookup_callable_origins(name).as_deref() {
            Some([signature]) => reject_unavailable(Some(signature)),
            _ => Ok(()),
        }
    }

    /// A call of a built-in or a struct construction: a name no binding
    /// shadows.
    fn infer_unbound_call(
        &self,
        span: SourceSpan,
        name: &str,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        match name {
            _ if self.structs.contains_key(name) => {
                let constructed = self.infer_construction(&span, name, param_args, args, kwargs)?;
                self.type_runtime_param_args(param_args, &self.recorded_bracket_attributes(&span))?;
                Ok(constructed)
            }
            "__get_mvalue_as_litref" => self.infer_mvalue_as_litref(span, param_args, args, kwargs),
            // Compiler-private tagged-union storage:
            // `__VariantStorage[T0, ...](value^)` selects the alternative
            // by the payload's type; `(init_with=factory)` by the
            // factory's result type.
            // An `enum`'s storage: `__EnumStorage[T0, ...](payload,
            // index=i)` builds case `i` from its payload, by index, since
            // cases may share a payload type.
            _ if name == mojito_types::types::ENUM_STORAGE_TYPE_NAME => {
                if !self.enum_storage_allowed() {
                    return Err(TypeError::Unsupported(format!(
                        "'{name}' is the storage of an 'enum'; declare one instead"
                    )));
                }
                self.infer_enum_storage_construction(span, param_args, args, kwargs)
            }
            _ if name == mojito_types::types::VARIANT_STORAGE_TYPE_NAME => {
                if !self.bundled_stdlib_declaration {
                    return Err(TypeError::Unsupported(format!(
                        "'{name}' is compiler-private storage; use Variant from std.utils"
                    )));
                }
                self.infer_variant_construction(span, param_args, args, kwargs)
            }
            // Compiler-private inline uninit storage: `__UninitStorage[T]()`
            // (uninitialized) or `__UninitStorage[T](value^)` (initialized),
            // reachable only from the bundled crossing module.
            _ if name == mojito_types::types::UNINIT_STORAGE_TYPE_NAME => {
                reject_kwargs(kwargs)?;
                self.infer_uninit_storage_construction(span, param_args, args)
            }
            // A well-formed `rebind` was erased before checking; one
            // that reaches here is malformed.
            "rebind" => Err(Self::rebind_shape_error(param_args, args)),
            // One the elaborator left is over a binder of a template
            // body (`materialize[names[i]]()`): the runtime value of its
            // compile-time operand, which MIR lowers in its place.
            "materialize" if args.is_empty() && kwargs.is_empty() => {
                self.infer_template_materialize(&span, param_args)
            }
            // One the elaborator left is over a binder of a template
            // body (`comptime(len(L))`): the runtime value of its
            // compile-time operand, as `materialize[...]()` is.
            "comptime" if args.len() == 1 && param_args.is_empty() && kwargs.is_empty() => {
                self.infer_template_comptime(&args[0])
            }
            // `conforms_to(T, Tr)`: a compile-time `Bool` read as a
            // runtime value, the parameter constant of its query.
            "conforms_to" if param_args.is_empty() && kwargs.is_empty() => {
                let [subject, traits] = args else {
                    return Err(TypeError::ArityMismatch {
                        name: "conforms_to".to_string(),
                        expected: 2,
                        got: args.len(),
                    });
                };
                let value = self.conformance_query(subject, traits)?;
                self.record_member_value(span, value);
                Ok(Ty::bool())
            }
            _ if !kwargs.is_empty() => Err(TypeError::BadCall {
                func: name.to_string(),
                reason: "keyword arguments are not supported here".to_string(),
            }),
            // Upstream's libc FFI spelling over Mojito's closed callee
            // table (`mojito_types::ffi`); the backends read the callee
            // from the call's instantiated arguments and the result type
            // from the destination register.
            "external_call" => self.infer_external_call(&span, param_args, args),
            // The static address of a parameter list's elements, spelled
            // only by the bundled `ParameterList` (upstream's
            // `global_constant` over `#pop.variadic_to_array`).
            PARAM_LIST_ADDRESS
                if super::overload_support::is_bundled_variadics_source(span.source.as_deref()) =>
            {
                self.infer_param_list_address(span, param_args, args)
            }
            // `_unqualified_type_name[T]()` (upstream `std.reflection`):
            // the `StaticString` of one checked type's unqualified
            // spelling, answered per instance over a symbolic type.
            "_unqualified_type_name" => {
                if param_args.len() != 1 {
                    return Err(TypeError::WrongTypeArgCount {
                        name: "_unqualified_type_name".to_string(),
                        expected: 1,
                        got: param_args.len(),
                    });
                }
                if !args.is_empty() {
                    return Err(TypeError::ArityMismatch {
                        name: "_unqualified_type_name".to_string(),
                        expected: 0,
                        got: args.len(),
                    });
                }
                let ty = self.type_param_argument(&param_args[0], "_unqualified_type_name")?;
                let attributes = DeclaredAttributes::of_builtin(
                    "_unqualified_type_name",
                    TYPE_PARAMS,
                    param_args,
                    types_attributes(param_args, std::slice::from_ref(&ty)),
                )?;
                self.record_declared_attributes(span.clone(), &attributes);
                self.reflected_static_name(
                    span,
                    &ty,
                    mojito_types::param_expr::TypeNameForm::Unqualified,
                )
            }
            // Compiler-private crossing for `std.os.abort`: an uncatchable
            // VM trap carrying a nominal String message. Typed as returning
            // None — the call never returns at runtime, and stdlib call
            // sites keep a normal control-flow path after it rather than
            // relying on bottom-type flow analysis.
            // The `with` desugar's liveness anchor: a named local kept
            // alive to this statement without a copy or a move (HIR
            // lowers the statement to `KeepAlive`).
            name if name == with_stmt::KEEP_ALIVE_BUILTIN => {
                let [argument] = args else {
                    return Err(TypeError::ArityMismatch {
                        name: name.to_string(),
                        expected: 1,
                        got: args.len(),
                    });
                };
                let ExprKind::Identifier(local) = &argument.kind else {
                    return Err(TypeError::BadCall {
                        func: name.to_string(),
                        reason: "the argument must name a local variable".to_string(),
                    });
                };
                if self.lookup(local).is_none() {
                    return Err(TypeError::UndefinedVariable(local.clone()));
                }
                self.infer(argument)?;
                Ok(Ty::none())
            }
            "_mojito_float_text" => self.infer_float_text(args),
            "_mojito_abort" => {
                let tys = self.builtin_args("_mojito_abort", 1, args)?;
                self.bind_read_builtin_argument(&span, &args[0], &tys[0]);
                match (tys[0]).kind() {
                    // A literal message avoids an os-module import cycle
                    // for stdlib-internal trap sites; the VM reads either
                    // spelling.
                    kind if kind.is_string_literal() => Ok(Ty::never()),
                    struct_ty
                        if let Some((name, targs)) = struct_ty.as_non_pointer_struct()
                            && targs.is_empty()
                            && mojito_symbol::symbol::is_stdlib_string_struct(name) =>
                    {
                        Ok(Ty::never())
                    }
                    other => Err(TypeError::TypeMismatch {
                        expected: "String".to_string(),
                        found: other.to_string(),
                        context: "argument to '_mojito_abort'".to_string(),
                    }),
                }
            }
            "abs" => self.infer_abs(args),
            "min" | "max" => self.infer_min_max(name, args),
            "input" => self.infer_input(span, args),
            "len" => self.infer_len(args),
            "range" => self.infer_range(args),
            "List" => self.infer_list_construction(&span, param_args, args),
            "Set" => {
                let (collection, attributes) = self.set_type(param_args)?;
                self.record_declared_attributes(span, &attributes);
                let element = set_element(&collection)
                    .expect("Set type helper returns a nominal Set")
                    .clone();
                for argument in args {
                    let actual = self.infer(argument)?;
                    if !self.coerces_by_origins(&actual, &element) {
                        return Err(TypeError::TypeMismatch {
                            expected: element.to_string(),
                            found: actual.to_string(),
                            context: "Set construction element".to_string(),
                        });
                    }
                    self.record_literal_materializations(argument, &actual, &element)?;
                    self.check_consuming(argument, &actual, "Set construction element")?;
                }
                Ok(self.nominal_set(element))
            }
            "Dict" => {
                if !args.is_empty() {
                    return Err(TypeError::ArityMismatch {
                        name: "Dict".to_string(),
                        expected: 0,
                        got: args.len(),
                    });
                }
                let (dict, attributes) = self.dict_type(param_args)?;
                self.record_declared_attributes(span, &attributes);
                Ok(dict)
            }
            "Tuple" => {
                let tuple = self.infer_tuple_construction(&span, param_args, args)?;
                if let Some((target, _)) = tuple.kind().as_non_pointer_struct()
                    && target != mojito_types::types::TUPLE_TYPE_NAME
                {
                    self.record_at(span).overload_target = Some(target.clone());
                }
                Ok(tuple)
            }
            // The enclosing struct's own parameter is spelled `Self.<name>`
            // inside the body; the bare spelling is upstream's error.
            _ if self.is_enclosing_struct_param(name) && self.lookup_tparam(name).is_none() => {
                Err(TypeError::UnqualifiedStructParam(name.to_string()))
            }
            _ => Err(TypeError::UndefinedVariable(name.to_string())),
        }
    }

    /// A construction the checker spelled for a type with no source
    /// spelling, bound to `instance`: a struct's `__init__` overloads solve
    /// its parameters against the instance, a `SIMD`'s as an alias's do.
    fn infer_instance_construction(
        &self,
        span: &SourceSpan,
        instance: &Ty,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        match instance.kind() {
            // A numeric literal's construction is its declared struct's.
            TyKind::Struct(name, _) if instance.is_numeric_literal() => {
                self.infer_construction_of(span, name, Some(instance), &[], args, kwargs)
            }
            kind if kind.as_simd().is_some() => self.infer_construction_of(
                span,
                mojito_types::types::SIMD_TYPE_NAME,
                Some(instance),
                &[],
                args,
                kwargs,
            ),
            // An origin slot the arguments leave open is the instance's.
            struct_ty if let Some((name, expected)) = struct_ty.as_non_pointer_struct() => self
                .infer_construction_of(span, name, Some(instance), &[], args, kwargs)
                .map(|constructed| match constructed.kind() {
                    struct_ty
                        if let Some((name, arguments)) = struct_ty.as_non_pointer_struct()
                            && arguments.len() == expected.len() =>
                    {
                        let arguments = arguments
                            .iter()
                            .zip(expected)
                            .map(|(argument, expected)| match (argument, expected) {
                                (
                                    TyArg::Origin(mojito_types::origin::Origin::Unbound),
                                    TyArg::Origin(_),
                                ) => expected.clone(),
                                _ => argument.clone(),
                            })
                            .collect();
                        TyKind::Struct(name.clone(), arguments).into()
                    }
                    _ => constructed,
                }),
            // A pointer's construction is its declared initializers'; its
            // bracket is the instance's.
            kind if kind.is_pointer() => {
                let constructed = self.infer_construction_of(
                    span,
                    POINTER_TYPE_NAME,
                    Some(instance),
                    &[],
                    args,
                    kwargs,
                )?;
                if self.coerces_by_origins(&constructed, instance) {
                    Ok(instance.clone())
                } else {
                    Err(TypeError::TypeMismatch {
                        expected: instance.to_string(),
                        found: constructed.to_string(),
                        context: "the construction of a 'Pointer'".to_string(),
                    })
                }
            }
            // A reflected field type's default construction.
            TyKind::Dependent(_) if args.is_empty() && kwargs.is_empty() => {
                self.construct_dependent(span.clone(), instance.clone())
            }
            _ => Err(TypeError::Unsupported(format!(
                "cannot emit initializer list for '{instance}': the type has no construction \
                 to spell"
            ))),
        }
    }

    /// Check every member of a free-function overload set against the
    /// call, each inside a probe: whatever a candidate records is undone,
    /// and only the selected candidate's re-run leaves facts. The call's
    /// nested calls and operator expressions, which type alike under every
    /// candidate, are typed once before the probes open, keeping their
    /// facts, as upstream emits each argument once and ranks the
    /// candidates over the typed values; the argument memo hands each
    /// candidate their types.
    fn probe_overload_candidates(
        &self,
        call: &OverloadCall<'_>,
        candidates: &[Ty],
        origin_signatures: &[CallableOriginSignature],
    ) -> OverloadProbes {
        for argument in call
            .args
            .iter()
            .chain(call.kwargs.iter().map(|kwarg| &kwarg.value))
            .filter(|argument| {
                matches!(
                    argument.kind,
                    ExprKind::Call { .. }
                        | ExprKind::Invoke { .. }
                        | ExprKind::MethodCall { .. }
                        | ExprKind::Infix(..)
                        | ExprKind::Prefix(..)
                )
            })
        {
            let _ = self.infer(argument);
        }
        let mut probes = OverloadProbes {
            matches: Vec::new(),
            availability_failures: Vec::new(),
            bound_failures: Vec::new(),
            argument_failure: None,
        };
        for (index, candidate) in candidates.iter().enumerate() {
            match self.probe(|| {
                self.check_overload_candidate(call, index, candidate, origin_signatures.get(index))
            }) {
                CandidateOutcome::Match(found) => probes.matches.push(found),
                CandidateOutcome::Unavailable(reason) => probes.availability_failures.push(reason),
                CandidateOutcome::Bound(failure) => probes.bound_failures.push(failure),
                CandidateOutcome::Argument(failure) => {
                    probes.argument_failure.get_or_insert(failure);
                }
                CandidateOutcome::Rejected => {}
            }
        }
        probes
    }

    /// Check one member of a free-function overload set against the call.
    fn check_overload_candidate(
        &self,
        call: &OverloadCall<'_>,
        index: usize,
        candidate: &Ty,
        signature: Option<&CallableOriginSignature>,
    ) -> CandidateOutcome {
        match self.infer_callable_ty(
            call.span,
            call.name,
            candidate.clone(),
            (call.param_args, signature),
            call.args,
            call.kwargs,
        ) {
            Ok((ret, score, error, _, simd_erasures, ..)) => {
                let Some(target) = callable_lowered_name(call.name, candidate) else {
                    return CandidateOutcome::Rejected;
                };
                CandidateOutcome::Match(CallableOverloadMatch {
                    ret,
                    score,
                    target,
                    index,
                    error,
                    owned: owned_parameters(candidate),
                    owned_arguments: owned_arguments(candidate, call.args, call.kwargs),
                    simd_erasures,
                })
            }
            Err(TypeError::BadCall { reason, .. })
                if reason.starts_with("constraint failed: ")
                    || reason.starts_with("violated constraint; ")
                    || reason.starts_with(LACKING_EVIDENCE) =>
            {
                // Generic constraints are validated while solving type
                // arguments, before the final coercion and alias checks.
                // Probe the same candidate without those availability
                // predicates so only an otherwise call-compatible shape
                // can contribute a retained diagnostic.
                let mut unconstrained = candidate.clone();
                unconstrained.update(|kind| {
                    if let Some(GenericFuncTy { decls, .. }) = kind.as_generic_func_mut() {
                        for constraints in decls.iter_mut().filter_map(ParamDecl::constraints_mut) {
                            constraints.clear();
                        }
                    }
                });
                if self
                    .infer_callable_ty(
                        call.span,
                        call.name,
                        unconstrained,
                        (call.param_args, signature),
                        call.args,
                        call.kwargs,
                    )
                    .is_ok()
                {
                    // A sole candidate's own failed assertion or unproven
                    // constraint is the call's diagnostic, as upstream
                    // reports it.
                    CandidateOutcome::Unavailable(
                        (reason.starts_with("constraint failed: ")
                            || reason.starts_with(LACKING_EVIDENCE))
                        .then_some(reason),
                    )
                } else {
                    CandidateOutcome::Rejected
                }
            }
            // A candidate whose only failure is a trait bound on a solved
            // type argument (`hash(x)` over a non-Hashable `x` against
            // `hash[T: Hashable, //, ...]`) keeps its conformance
            // diagnostic, like a single callable's.
            Err(failure @ TypeError::TraitNotSatisfied { .. }) => CandidateOutcome::Bound(failure),
            // An argument with no runtime form fails every candidate alike.
            Err(failure @ TypeError::ComptimeCrossing(_)) => CandidateOutcome::Argument(failure),
            Err(_) => CandidateOutcome::Rejected,
        }
    }

    /// The result of a builtin that builds the nominal `String` itself
    /// (`input`), as upstream's function returns one: the stdlib struct.
    /// An unlinked seam without it has no type for a run-time string.
    pub(super) fn builtin_string_result(&self, builtin: &str) -> Result<Ty, TypeError> {
        let name = mojito_symbol::symbol::STDLIB_STRING_STRUCT;
        self.structs
            .contains_key(name)
            .then(|| Ty::new(TyKind::Struct(name.to_string(), Vec::new())))
            .ok_or_else(|| {
                TypeError::Unsupported(format!(
                    "'{builtin}' without the bundled 'String' to return"
                ))
            })
    }

    /// The type a value of `ty` materializes to where it becomes a run-time
    /// value: the target its register-passable struct's
    /// `@__nonmaterializable(T)` names, as upstream's materialization reads
    /// the attribute. `None` for a materializable type, and on a seam that
    /// links no declaration of the literal's struct.
    pub(super) fn materialization_target(&self, ty: &Ty) -> Option<Ty> {
        let name = match ty.kind() {
            TyKind::Struct(name, _) => name.as_str(),
            _ => return None,
        };
        let target = self.structs.get(name)?.materializes_to.clone()?;
        (self.register_passability(ty) == super::traits::RegisterPassability::Proven)
            .then_some(target)
    }

    /// The type a type parameter deduced from an argument of type `actual`
    /// binds: the materialization target, else `actual` with its literal
    /// arguments defaulted.
    pub(super) fn deduced_ty(&self, actual: &Ty) -> Ty {
        self.materialization_target(actual)
            .filter(|_| self.materializes_by_construction(actual))
            .unwrap_or_else(|| default_literal(actual))
    }

    /// The type an argument of type `actual` passes at a parameter typed
    /// `param`: an existential `Some[Trait]` is the anonymous `[T: Trait]`
    /// binder upstream desugars it to, which the argument deduces through
    /// [`Self::deduced_ty`] (a literal materializes or defaults) when the
    /// deduced type conforms. Any other parameter, or a deduction that does
    /// not conform, keeps `param`.
    pub(super) fn existential_argument_ty(&self, param: &Ty, actual: &Ty) -> Ty {
        let TyKind::Param {
            binder,
            callable_bound: None,
            ..
        } = param.kind()
        else {
            return param.clone();
        };
        if !is_existential_binder(binder) {
            return param.clone();
        }
        let deduced = self.deduced_ty(actual);
        if self.value_coerces(&deduced, param) {
            deduced
        } else {
            param.clone()
        }
    }

    /// `ty` with each existential binder `existentials` solves
    /// ([`bind_existential`]) replaced by its argument's type, and the members
    /// it then reads closed (`List[Int].IteratorOwnedType`).
    pub(super) fn close_existentials(&self, ty: Ty, existentials: &TySubst) -> Ty {
        if existentials.is_empty() {
            ty
        } else {
            self.resolve_assoc_ty(&substitute(&ty, existentials))
        }
    }

    /// Whether a value of `ty` materializes through its target's implicit
    /// constructor (a `StringLiteral`, a user struct), rather than by the
    /// compiler's literal folding (`IntLiteral`, `FloatLiteral`).
    pub(super) fn materializes_by_construction(&self, ty: &Ty) -> bool {
        !ty.is_numeric_literal() && self.materialization_target(ty).is_some()
    }

    /// Materialize the value of type `ty` at `span` as a run-time value (an
    /// inferred binding, a display element): record its target's implicit
    /// constructor at `span` and report the target, or keep `ty` when it
    /// does not materialize or the target declares no such constructor.
    pub(super) fn materialize_at(&self, ty: &Ty, span: SourceSpan) -> Result<Ty, TypeError> {
        let Some(target) = self
            .materialization_target(ty)
            .filter(|_| self.materializes_by_construction(ty))
        else {
            return Ok(ty.clone());
        };
        let Some(selected) = self.implicit_conversion_constructor(ty, &target)? else {
            return Ok(ty.clone());
        };
        // The constructor's own parameters solved from the value (`String`'s
        // `__init__(data: StringLiteral)` binds the literal's `value`) are
        // its call's arguments, as any conversion passes them.
        let instantiation = self.conversion_instantiation(
            &target,
            &selected.target,
            (
                &selected.own_decls,
                selected.own_arguments,
                &selected.own_mutabilities,
            ),
        );
        // A view conversion already recorded at this span supersedes the
        // materialization; re-inferring the expression must not revert it.
        let mut facts = self.record_at(span);
        if facts
            .implicit_conversion
            .get_or_insert_with(|| selected.target.clone())
            == &selected.target
            && !instantiation.is_empty()
        {
            facts.conversion_instantiation.get_or_insert(instantiation);
        }
        Ok(target)
    }

    /// `T(copy=x)` on a type parameter: construction through the bound's
    /// `Copyable` initializer, `__init__(out self, *, copy: Self)`, which
    /// borrows a value of the parameter's own type. Recorded as the
    /// `ConstructTypeParam` of the binder; its argument stays the call's own.
    pub(super) fn infer_type_param_copy_construction(
        &self,
        span: SourceSpan,
        name: &str,
        ty: &Ty,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        let TyKind::Param { binder, bounds, .. } = ty.kind() else {
            return Err(TypeError::InvariantViolation(format!(
                "constructing '{name}' names no type parameter"
            )));
        };
        // A nullary requirement (`Defaultable`'s) is `T()`'s, never this
        // call's.
        if let Some(bound) = bounds.iter().find(|bound| {
            self.traits
                .get(bound.as_str())
                .and_then(|info| info.methods.get("__init__"))
                .is_some_and(|inits| inits.iter().any(|init| !init.params.is_empty()))
        }) {
            return Err(TypeError::Unsupported(format!(
                "constructing type parameter '{name}' through the `__init__` requirement \
                 of trait '{bound}'"
            )));
        }
        let ([], [], [source]) = (param_args, args, kwargs) else {
            return Err(TypeError::BadCall {
                func: name.to_string(),
                reason: "a type parameter is constructed only through its bound's \
                         initializers, `()` or `(copy=value)`"
                    .to_string(),
            });
        };
        if source.name != "copy" {
            return Err(TypeError::BadCall {
                func: name.to_string(),
                reason: format!(
                    "no initializer of the parameter's bound takes keyword '{}'",
                    source.name
                ),
            });
        }
        if !self.is_copyable(ty) {
            return Err(TypeError::TraitNotSatisfied {
                param: name.to_string(),
                ty: ty.to_string(),
                trait_name: "Copyable".to_string(),
                reason: self.trait_failure_reason(ty, "Copyable"),
            });
        }
        let found = self.infer(&source.value)?;
        if !mojito_types::types::coerces(&found, ty) {
            return Err(TypeError::TypeMismatch {
                expected: ty.to_string(),
                found: found.to_string(),
                context: format!("argument 'copy' of '{name}'"),
            });
        }
        // The source binds the bound's `read copy: Self`, judged at the
        // parameter as the requirement's signature declares it.
        self.record_operand_convention(
            &span,
            mojito_checked::checked::CheckedCallArgumentSource::Keyword(0),
            None,
            self.memory_only(ty),
        );
        self.record_at(span).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::ConstructTypeParam {
                param: binder.clone(),
            },
        ));
        Ok(ty.clone())
    }

    /// The `copy: Self` argument of a synthesized copy initializer at `ty`,
    /// converted to `ty` as any argument is; answers the argument's own type.
    pub(super) fn copy_argument(
        &self,
        name: &str,
        ty: &Ty,
        source: &mojito_ast::ast::KwArg,
    ) -> Result<Ty, TypeError> {
        let found = self.infer_with_expected(&source.value, ty, true)?;
        if !self.record_implicit_conversion(&source.value, &found, ty)? {
            return Err(TypeError::TypeMismatch {
                expected: ty.to_string(),
                found: found.to_string(),
                context: format!("argument 'copy' of '{name}'"),
            });
        }
        Ok(found)
    }

    /// Type the call `call` of the value `name` as its method call
    /// `name.__call__(args)` ([`Self::infer_call_method`]). `None` for a
    /// callee no binding names, and for a nested `def` named by its
    /// declaration, whose call by name is a direct call of its lifted body;
    /// the call by name types both.
    pub(super) fn infer_value_method_call(
        &self,
        call: &Expr,
        name: &str,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<Result<Ty, TypeError>> {
        self.binding_scope(name)?;
        let ty = self.lookup(name)?;
        if super::is_closure_storage(ty)
            && self
                .lookup_owner(name)
                .is_some_and(|owner| self.nested_function_owners.contains(&owner))
        {
            return None;
        }
        let receiver = super::initializer_list::spelled_node(
            call,
            1,
            ExprKind::Identifier(name.to_string()),
            call.span,
        );
        if param_args.is_empty() {
            return self.infer_call_method(call, receiver, ty, args, kwargs);
        }
        self.infer_element_call_method(call, receiver.clone(), ty, param_args, args, kwargs)
            .or_else(|| {
                self.infer_bracketed_call_method(call, receiver, ty, param_args, args, kwargs)
            })
    }

    /// Type the call `value[params](args)` of a struct value declaring
    /// `__call__` and none of `__getitem__`, `__setitem__`, and
    /// `__getattr__` as the parameterized method call
    /// `value.__call__[params](args)`, as upstream's `SubscriptNode` binds
    /// such a bracket to `__call__`'s parameters. `None` for any other
    /// value.
    pub(super) fn infer_bracketed_call_method(
        &self,
        call: &Expr,
        receiver: Expr,
        ty: &Ty,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<Result<Ty, TypeError>> {
        let (struct_name, _) = ty.kind().as_non_pointer_struct()?;
        let info = self.structs.get(struct_name)?;
        if !info.methods.contains_key("__call__")
            || ["__getitem__", "__setitem__", "__getattr__"]
                .iter()
                .any(|method| info.methods.contains_key(method))
        {
            return None;
        }
        let callee = super::initializer_list::spelled_node(
            call,
            3,
            ExprKind::Member {
                object: Box::new(receiver),
                field: "__call__".to_string(),
            },
            call.span,
        );
        let spelled = super::initializer_list::spelled_node(
            call,
            0,
            ExprKind::Invoke {
                callee: Box::new(callee),
                param_args: param_args.to_vec(),
                args: args.to_vec(),
                kwargs: kwargs.to_vec(),
            },
            call.span,
        );
        Some(self.infer_spelled(call, spelled))
    }

    /// Type the call `base[i, …](args)` of a subscripted struct value as
    /// the element's method call `base[i, …].__call__(args)`
    /// ([`Self::infer_call_method`]): `None` unless the bracket holds
    /// run-time indices over a struct declaring `__getitem__` and no
    /// callable contract, and its element declares `__call__`.
    pub(super) fn infer_element_call_method(
        &self,
        call: &Expr,
        base: Expr,
        base_ty: &Ty,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<Result<Ty, TypeError>> {
        let indices = param_args
            .iter()
            .map(|argument| match argument {
                mojito_ast::ast::ParamArg::Value(value) => Some(value.clone()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;
        let (struct_name, _) = base_ty.kind().as_non_pointer_struct()?;
        if self.declared_callable_contract(base_ty).is_some()
            || !self
                .structs
                .get(struct_name)
                .is_some_and(|info| info.methods.contains_key("__getitem__"))
        {
            return None;
        }
        let subscript = match <[Expr; 1]>::try_from(indices) {
            Ok([index]) => ExprKind::Index {
                object: Box::new(base),
                index: Box::new(index),
            },
            Err(indices) => ExprKind::MultiIndex {
                object: Box::new(base),
                args: indices
                    .into_iter()
                    .map(mojito_ast::ast::SubscriptArg::Index)
                    .collect(),
            },
        };
        let element = super::initializer_list::spelled_node(call, 2, subscript, call.span);
        let element_ty = self.infer(&element).ok()?;
        self.infer_call_method(call, element, &element_ty, args, kwargs)
    }

    /// Whether `object.name` reads a struct's callable value parameter:
    /// `object` is `Self` or a struct type (`S[seven]`), and `name` one of
    /// its parameters of a callable type.
    pub(super) fn is_struct_callable_parameter(&self, object: &Expr, name: &str) -> bool {
        let declares = |decls: &[ParamDecl]| {
            decls.iter().any(|decl| {
                matches!(decl, ParamDecl::Value { name: parameter, ty, .. }
                    if parameter == name
                        && matches!(ty.kind(), TyKind::Func(_) | TyKind::GenericFunc(_)))
            })
        };
        match &object.kind {
            ExprKind::Identifier(spelled) if spelled == "Self" => {
                self.lookup("Self").is_none() && declares(&self.self_decls)
            }
            _ => self
                .member_type_operand(object)
                .and_then(|ty| {
                    let (struct_name, _) = ty.kind().as_non_pointer_struct()?;
                    self.structs
                        .get(struct_name)
                        .map(|info| declares(&info.decls()))
                })
                .unwrap_or(false),
        }
    }

    /// Type the call `Self.f(args)` (or `S[g].f(args)`) of a struct's
    /// callable value parameter `f` as the call of the value `Self.f`, as the pin
    /// calls the `#kgen.func.symbol` it binds. The checked arena hands HIR
    /// the spelled `(Self.f)(args)` in the call's place. `None` when `f`
    /// is no such parameter.
    pub(super) fn infer_struct_callable_parameter_call(
        &self,
        call: &Expr,
        object: &Expr,
        parameter: &str,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<Result<Ty, TypeError>> {
        if !self.is_struct_callable_parameter(object, parameter) {
            return None;
        }
        let callee = super::initializer_list::spelled_node(
            call,
            1,
            ExprKind::Member {
                object: Box::new(object.clone()),
                field: parameter.to_string(),
            },
            call.span,
        );
        let spelled = super::initializer_list::spelled_node(
            call,
            0,
            ExprKind::Invoke {
                callee: Box::new(callee),
                param_args: Vec::new(),
                args: args.to_vec(),
                kwargs: kwargs.to_vec(),
            },
            call.span,
        );
        Some(self.infer(&spelled).inspect(|ty| {
            self.record_spelled_construction(call, spelled, ty);
        }))
    }

    /// Type the call `object.field(args)` of a struct field no method
    /// answers as the field's method call `object.field.__call__(args)`
    /// ([`Self::infer_call_method`]). `None` when `object` has no such field.
    pub(super) fn infer_field_call_method(
        &self,
        call: &Expr,
        object: &Expr,
        field: &str,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<Result<Ty, TypeError>> {
        let object_ty = self.infer(object).ok()?;
        let (struct_name, arguments) = object_ty.kind().as_non_pointer_struct()?;
        let info = self.structs.get(struct_name)?;
        let (_, field_ty) = info.fields.iter().find(|(name, _)| name == field)?;
        let field_ty = self.substitute_at(field_ty, info, arguments);
        let receiver = super::initializer_list::spelled_node(
            call,
            1,
            ExprKind::Member {
                object: Box::new(object.clone()),
                field: field.to_string(),
            },
            call.span,
        );
        self.infer_call_method(call, receiver, &field_ty, args, kwargs)
    }

    /// Type the call `call` of `receiver`, a value of type `ty`, as its
    /// method call `receiver.__call__(args)`, as upstream's `CallNode`
    /// resolves a callee that is no function: a struct value declaring
    /// `__call__`, or a value of a type parameter whose bounds declare it.
    /// The checked arena hands HIR the spelled method call in the call's
    /// place. `None` for any other callee.
    pub(super) fn infer_call_method(
        &self,
        call: &Expr,
        receiver: Expr,
        ty: &Ty,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<Result<Ty, TypeError>> {
        let bound = matches!(
            ty.kind(),
            TyKind::Param {
                callable_bound: None,
                ..
            }
        );
        let declared = ty
            .kind()
            .as_non_pointer_struct()
            .is_some_and(|(struct_name, _)| {
                self.structs
                    .get(struct_name)
                    .is_some_and(|info| info.methods.contains_key("__call__"))
            });
        if !bound && !declared {
            return None;
        }
        let spelled = super::initializer_list::spelled_node(
            call,
            0,
            ExprKind::MethodCall {
                object: Box::new(receiver),
                method: "__call__".to_string(),
                args: args.to_vec(),
                kwargs: kwargs.to_vec(),
            },
            call.span,
        );
        match self.infer(&spelled) {
            // A type parameter whose bounds declare no `__call__`.
            Err(TypeError::NoSuchMethod { .. }) if bound => None,
            result => Some(result.inspect(|ty| {
                self.record_spelled_construction(call, spelled, ty);
            })),
        }
    }

    #[allow(clippy::too_many_lines, reason = "TODO: split this pass")]
    pub(super) fn infer_call(
        &self,
        span: SourceSpan,
        name: &str,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        let spelled = self.spelled_instances.borrow().get(&span).cloned();
        if let Some(instance) = spelled {
            return self.infer_instance_construction(&span, &instance, args, kwargs);
        }
        if name == "__RuntimeTuple"
            && (self.bundled_stdlib_declaration
                || super::overload_support::is_bundled_module_source(span.source.as_deref()))
            && param_args.is_empty()
            && kwargs.is_empty()
        {
            return args
                .iter()
                .map(|argument| self.infer(argument))
                .collect::<Result<Vec<_>, _>>()
                .map(|elements| TyKind::Tuple(elements).into());
        }
        if let Some(element) = self.infer_pack_element_construction(
            span.clone(),
            name,
            false,
            param_args,
            args,
            kwargs,
        ) {
            return element;
        }
        if param_args.is_empty() {
            let type_parameter = self
                .param_scopes
                .borrow()
                .types
                .iter()
                .rev()
                .find_map(|scope| scope.get(name).cloned())
                .or_else(|| {
                    self.self_decls
                        .iter()
                        .filter(|declaration| declaration.name() == name)
                        .find_map(type_parameter)
                })
                // A trait member's `Self` is its `_Self` binder.
                .or_else(|| {
                    self.self_ty
                        .clone()
                        .filter(|ty| name == "Self" && matches!(ty.kind(), TyKind::Param { .. }))
                });
            if let Some(ty) = type_parameter.as_ref()
                && let TyKind::Param { binder, bounds, .. } = ty.kind()
                && (!args.is_empty()
                    || !kwargs.is_empty()
                    || bounds
                        .iter()
                        .any(|bound| self.bound_default_constructs(bound)))
            {
                if !(args.is_empty() && kwargs.is_empty()) {
                    return self.infer_type_param_copy_construction(
                        span,
                        &binder.name,
                        ty,
                        param_args,
                        args,
                        kwargs,
                    );
                }
                self.record_at(span).adjustment = Some(Box::new(
                    mojito_checked::checked::SemanticAdjustment::ConstructTypeParam {
                        param: binder.clone(),
                    },
                ));
                return Ok(ty.clone());
            }
        }
        self.reject_nested_self_reference(name)?;
        let Some(ty) = self.lookup(name).cloned() else {
            return self.infer_unbound_call(span, name, param_args, args, kwargs);
        };
        // A call through a compile-time function value (`comptime h = sq`,
        // `h(7)`) is the call of the function it folds to, `sq(7)`, never
        // of the binding.
        let folded = self
            .folded_function_callee(name)
            .filter(|function| function.symbol != name);
        if let Some(function) = folded.as_ref().filter(|function| {
            function.is_bare() && self.binding_scope(&function.symbol) == Some(0)
        }) {
            let called =
                self.infer_call(span.clone(), &function.symbol, param_args, args, kwargs)?;
            self.record_at(span)
                .overload_target
                .get_or_insert_with(|| function.symbol.clone());
            return Ok(called);
        }
        self.record_permitted_call_capture(name);
        if let Some(owner) = self.lookup_owner(name) {
            self.record_at(span.clone()).binding = Some(owner);
        }
        let origin_signatures = self.lookup_callable_origins(name).unwrap_or_default();
        if let TyKind::Overload(candidates) = ty.kind() {
            return self.with_argument_memo(|| {
                let OverloadProbes {
                    matches,
                    availability_failures,
                    bound_failures,
                    argument_failure,
                } = self.probe_overload_candidates(
                    &OverloadCall {
                        span: &span,
                        name,
                        param_args,
                        args,
                        kwargs,
                    },
                    candidates,
                    &origin_signatures,
                );
                match select_callable_overload(matches) {
                    Ok((ret, target, selected_index, error)) => {
                        reject_unavailable(origin_signatures.get(selected_index))?;
                        self.record_at(span.clone()).overload_target = Some(target);
                        if let Some((index, selected)) = candidates
                            .get(selected_index)
                            .map(|selected| (selected_index, selected))
                        {
                            let (.., attributes, prepared) = self.infer_callable_ty(
                                &span,
                                name,
                                selected.clone(),
                                (param_args, origin_signatures.get(index)),
                                args,
                                kwargs,
                            )?;
                            self.record_declared_attributes(span.clone(), &attributes);
                            self.record_call_environment_effects(
                                span.clone(),
                                &prepared,
                                param_args,
                                &attributes.source_order(),
                                args,
                                kwargs,
                            )?;
                            // Overloads share the bare-name effect entry (a
                            // conservative union); replay it and any call-through
                            // residues exactly like the single-callable path.
                            let bindings = self.call_bindings.borrow_mut().remove(&span);
                            self.apply_transfer_effects(
                                name,
                                None,
                                args,
                                &span,
                                &bindings.unwrap_or_default(),
                            )?;
                            self.record_call_through(name, &prepared, args);
                            self.record_call_parameter_names(&span, &prepared);
                            let callee_decls = match prepared.kind() {
                                TyKind::GenericFunc(generic) => generic.argument_view(),
                                _ => std::borrow::Cow::Borrowed(&[][..]),
                            };
                            self.apply_call_through_effects(
                                name,
                                &callee_decls,
                                None,
                                param_args,
                                args,
                                &span,
                            )?;
                            self.record_free_call_result_origins(
                                &span,
                                &prepared,
                                origin_signatures.get(index),
                                args,
                                kwargs,
                            );
                        }
                        if let Some(error) = error.filter(|ty| *ty != TyKind::Never) {
                            self.require_call_error(
                                span.clone(),
                                format!("call to raising function '{name}'"),
                                error,
                            )?;
                        }
                        self.record_view_result_borrow(&span, &ret);
                        Ok(self.bind_call_result_origins(&span, ret))
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
                            && candidates.iter().filter_map(Ty::as_func).any(|func| {
                                func.ret.kind().as_non_pointer_struct().is_some_and(
                                    |(target, _)| {
                                        mojito_types::types::SCALAR_RANGE_FAMILY
                                            .iter()
                                            .any(|family| target.contains(family))
                                    },
                                )
                            })
                            && let Some(ty) = self.infer_scalar_range(&span, args)?
                        {
                            return Ok(ty);
                        }
                        if let Some(failure) = argument_failure {
                            return Err(failure);
                        }
                        if availability_failures.is_empty()
                            && let [failure] = bound_failures.as_slice()
                        {
                            return Err(failure.clone());
                        }
                        Err(TypeError::BadCall {
                            func: mojito_common::error::declared_spelling(name).to_string(),
                            reason: match availability_failures.as_slice() {
                                [Some(reason)] => reason.clone(),
                                _ => "no overload matches the supplied arguments".to_string(),
                            },
                        })
                    }
                    Err(OverloadSelect::Ambiguous) => Err(TypeError::BadCall {
                        func: mojito_common::error::declared_spelling(name).to_string(),
                        reason: "ambiguous overloaded call".to_string(),
                    }),
                }
            });
        }
        // `objs[0](3)` / `grid[i, j](x)`: value brackets over an indexable
        // runtime binding are a subscript, not compile-time parameter
        // application — current Mojo dispatches the element call. A struct
        // carrying its own callable contract keeps parameter application
        // (specialized callable values), and non-value bracket forms keep the
        // parenthesization hint in `infer_callable_ty`.
        if let Some((struct_name, _)) = ty.kind().as_non_pointer_struct()
            && !param_args.is_empty()
            && param_args
                .iter()
                .all(|argument| matches!(argument, mojito_ast::ast::ParamArg::Value(_)))
            && self.declared_callable_contract(&ty).is_none()
            && self
                .structs
                .get(struct_name)
                .is_some_and(|info| info.methods.contains_key("__getitem__"))
        {
            let indices: Vec<Expr> = param_args
                .iter()
                .filter_map(|argument| match argument {
                    mojito_ast::ast::ParamArg::Value(value) => Some(value.clone()),
                    _ => None,
                })
                .collect();
            let receiver = Expr {
                kind: ExprKind::Identifier(name.to_string()),
                span: span.span,
                source: span.source.clone(),
                syntax_id: mojito_common::token::SyntaxId::fresh(),
            };
            return self.infer_element_call(span, &receiver, &ty, &indices, args, kwargs);
        }
        reject_unavailable(origin_signatures.first())?;
        let indirect_target = match ty.kind() {
            struct_ty if let Some(_) = struct_ty.as_non_pointer_struct() => {
                self.indirect_callable_target(&ty)
            }
            _ if callable_contract_ty(&ty).is_some()
                && self.binding_scope(name).is_some_and(|scope| scope > 0) =>
            {
                self.indirect_callable_target(&ty)
            }
            _ => None,
        };
        let contract = self.closure_contract(&ty).unwrap_or_else(|| ty.clone());
        if indirect_target.is_some() && matches!(contract.kind(), TyKind::GenericFunc(_)) {
            let (contract, arguments) =
                self.instantiate_generic_callable_value(name, contract, param_args)?;
            self.record_at(span.clone()).adjustment = Some(Box::new(
                mojito_checked::checked::SemanticAdjustment::InstantiatedCallableContract {
                    contract,
                    arguments,
                },
            ));
        }
        super::type_resolution::reject_trait_callable_call(&ty)?;
        let (ret, _, error, _, _, attributes, ty) = self.infer_callable_ty(
            &span,
            name,
            ty,
            (param_args, origin_signatures.first()),
            args,
            kwargs,
        )?;
        self.record_declared_attributes(span.clone(), &attributes);
        // Replay the callee's loan-transfer effects against the actuals.
        let bindings = self.call_bindings.borrow_mut().remove(&span);
        self.apply_transfer_effects(name, None, args, &span, &bindings.unwrap_or_default())?;
        // A call through one of the current body's own callable parameters
        // records a higher-order residue for this body's callers to resolve;
        // a call to a callee WITH such residues resolves them against the
        // concrete callables this call supplies.
        self.record_call_through(name, &ty, args);
        let callee_decls = match ty.kind() {
            TyKind::GenericFunc(generic) => generic.argument_view(),
            _ => std::borrow::Cow::Borrowed(&[][..]),
        };
        self.apply_call_through_effects(name, &callee_decls, None, param_args, args, &span)?;
        // A function-typed VALUE carries its origin def's effects on the
        // type; replay those too (the name-keyed entry above covers direct
        // and nested calls by declaration name).
        let carried = contract_transfer_effects(&ty);
        if !carried.is_empty() {
            self.replay_transfer_effects(carried, None, args, &span, &TySubst::new())?;
        }
        // A callable struct value dispatches `Struct.__call__`; replay its
        // effects with the callee binding as the receiver actual.
        if let Some((struct_name, _)) = ty.kind().as_non_pointer_struct() {
            let callee_value = Expr {
                kind: ExprKind::Identifier(name.to_string()),
                span: span.span,
                source: span.source.clone(),
                syntax_id: mojito_common::token::SyntaxId::fresh(),
            };
            let method_key = format!("{struct_name}.__call__");
            let effect_key = indirect_target.as_deref().unwrap_or(&method_key);
            self.apply_transfer_effects(
                effect_key,
                Some(&callee_value),
                args,
                &span,
                &TySubst::new(),
            )?;
        }
        self.record_call_environment_effects(
            span.clone(),
            &ty,
            param_args,
            &attributes.source_order(),
            args,
            kwargs,
        )?;
        if let Some(target) = indirect_target {
            self.record_at(span.clone()).overload_target = Some(target);
        }
        self.record_view_result_borrow(&span, &ret);
        if let TyKind::GenericFunc(generic) = ty.kind() {
            self.record_at(span.clone()).declared_result = Some(generic.ret.clone());
        }
        self.record_free_call_result_origins(&span, &ty, origin_signatures.first(), args, kwargs);
        self.record_call_parameter_names(&span, &ty);
        // A call through a module constant's function value (`comptime g =
        // twice[4]`, `g()`, or an overload its annotation selected), typed
        // by the selected signature, is the call of that function, as
        // upstream folds a call of a `#kgen.func.symbol` constant; a local
        // one calls its slot.
        if let Some(function) = folded.filter(|_| self.binding_scope(name) == Some(0)) {
            let arguments = self.folded_call_arguments(&span, &ty, &function);
            let mut record = self.record_at(span.clone());
            record.overload_target = Some(function.symbol.clone());
            if !function.is_bare() {
                record.generic_instantiation =
                    Some(mojito_checked::checked::GenericInstantiation {
                        callee: function.symbol,
                        parameter_names: Vec::new(),
                        parameter_types: Vec::new(),
                        variadic: None,
                        arguments,
                    });
            }
        }
        let ret = self.bind_call_result_origins(&span, ret);
        if let Some(error) = error.filter(|ty| *ty != TyKind::Never) {
            self.require_call_error(span, format!("call to raising function '{name}'"), error)?;
        }
        Ok(ret)
    }

    /// The whole declared list of `function`, the symbol a module constant
    /// folds to, as the call at `span` through that constant's value of
    /// type `value` binds it: the bracket's arguments, and in each origin
    /// slot the bracket left open, the origin this call inferred for the
    /// generator `value` is over it.
    fn folded_call_arguments(
        &self,
        span: &SourceSpan,
        value: &Ty,
        function: &mojito_types::ct::FuncSymbolRef,
    ) -> Vec<TyArg> {
        let open = value.as_generic_func().map(|generic| &generic.decls);
        let declared = self
            .lookup(&function.symbol)
            .and_then(Ty::as_generic_func)
            .map(|generic| generic.decls.clone());
        let inferred = self.fact(span, |facts| {
            facts
                .generic_instantiation
                .as_ref()
                .map(|instantiation| &instantiation.arguments)
        });
        let (Some(open), Some(declared), Some(inferred)) = (open, declared, inferred) else {
            return function.arguments.clone();
        };
        if function.arguments.len() + open.len() != declared.len() {
            return function.arguments.clone();
        }
        let mut bound = function.arguments.iter();
        declared
            .iter()
            .filter_map(|decl| {
                open.iter()
                    .position(|candidate| candidate.id() == decl.id())
                    .map_or_else(|| bound.next(), |slot| inferred.get(slot))
                    .cloned()
            })
            .collect()
    }

    /// Record a direct callable's runtime parameters for the call at `span`;
    /// a callable struct value records none.
    pub(super) fn record_call_parameter_names(&self, span: &SourceSpan, callee: &Ty) {
        if let Some(FuncTy {
            names,
            params,
            conventions,
            ..
        }) = callee.callable()
        {
            self.call_parameters.borrow_mut().insert(
                span.clone(),
                names
                    .iter()
                    .zip(params)
                    .zip(conventions)
                    .map(|((name, ty), convention)| super::CallParameter {
                        name: name.clone(),
                        convention: *convention,
                        ty: ty.clone(),
                    })
                    .collect(),
            );
        }
    }

    /// A non-method call whose result is a ref-field struct (a borrowing
    /// view/iterator) lends its borrowed sources to the result, exactly as the
    /// method-receiver rule in `infer_method_call` does: record the
    /// view-result adjustment so lowering anchors the result and establishes
    /// caller-side loans. Reference results carry their own loan channel, and
    /// an already-recorded adjustment (captures, instantiated contracts) wins.
    fn record_view_result_borrow(&self, span: &SourceSpan, ret: &Ty) {
        // Reference fields and origin-bearing pointer fields (upstream's
        // `Pointer[T, Self.o]` iterator storage) both borrow.
        if ret.kind().is_non_pointer_struct() && self.type_carries_loans(ret) {
            self.record_at(span.clone())
                .adjustment
                .get_or_insert_with(|| {
                    Box::new(
                        mojito_checked::checked::SemanticAdjustment::BorrowViewResult {
                            materialized: None,
                            interior: Vec::new(),
                        },
                    )
                });
        }
    }

    /// Resolve a free call's view-return contract (the selected overload's
    /// `CallableOriginSignature::view_return`) against its arguments and
    /// record the result's origin slots; see `record_call_result_origins`.
    /// The slots are matched exactly as the call typing matched them.
    fn record_free_call_result_origins(
        &self,
        span: &SourceSpan,
        callee: &Ty,
        signature: Option<&super::CallableOriginSignature>,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) {
        let Some(signature) = signature.filter(|signature| !signature.view_return.is_empty())
        else {
            return;
        };
        let Some(FuncTy {
            names,
            required,
            variadic,
            kw_variadic,
            positional_only,
            keyword_only,
            ..
        }) = callee.callable()
        else {
            return;
        };
        let kw_names: Vec<&str> = kwargs
            .iter()
            .filter(|argument| !argument.is_forwarded())
            .map(|argument| argument.name.as_str())
            .collect();
        let Ok(matched) = match_call_slots(
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
        ) else {
            return;
        };
        self.record_call_result_origins(
            span,
            &signature.view_return,
            &matched.slots,
            args,
            kwargs,
            None,
        );
    }

    /// Type the bare element-call spelling — `objs[0](3)`, `a.b[i](x)`,
    /// `grid[i, j](x)` — as subscript-then-indirect-call, matching current
    /// Mojo. The brackets are a runtime subscript of `receiver`, an indexable
    /// value with no callable contract of its own. The selected `__getitem__`
    /// moves out of the call's `selected_call` fact into the
    /// `ElementInvocation` adjustment so node-level consumers
    /// (reference-result diversion, resolved-callable projection, raise
    /// attribution) see the element call, not the subscript read.
    pub(super) fn infer_element_call(
        &self,
        span: SourceSpan,
        receiver: &Expr,
        receiver_ty: &Ty,
        indices: &[Expr],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        let label = match &receiver.kind {
            ExprKind::Identifier(name) => name.clone(),
            ExprKind::Member { field, .. } => field.clone(),
            _ => "<element>".to_string(),
        };
        for (position, index) in indices.iter().enumerate() {
            self.prepare_index_argument(receiver_ty, index, "__getitem__", position)?;
        }
        let element_ty =
            self.infer_struct_getitem_call(span.clone(), receiver, indices, receiver_ty)?;
        let getter = self
            .clear_fact(&span, |facts| facts.selected_call.take().map(|fact| *fact))
            .ok_or_else(|| {
                TypeError::InvariantViolation(
                    "element-call subscript selection recorded no contract".to_string(),
                )
            })?;
        let Some(callable) = self.callable_value_contract(&element_ty) else {
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
                self.record_at(span.clone()).overload_target = Some(target.clone());
            }
            None => {
                self.clear_fact(&span, |facts| facts.overload_target.take());
            }
        }
        let (ret, _, error, ..) = self.infer_callable_ty(
            &span,
            "<element>",
            callable.clone(),
            (&[], None),
            args,
            kwargs,
        )?;
        let carried = contract_transfer_effects(&callable);
        if !carried.is_empty() {
            self.replay_transfer_effects(carried, None, args, &span, &TySubst::new())?;
        }
        if let Some((struct_name, _)) = element_ty.kind().as_non_pointer_struct() {
            let method_key = format!("{struct_name}.__call__");
            let effect_key = target.as_deref().unwrap_or(&method_key);
            self.apply_transfer_effects(effect_key, Some(receiver), args, &span, &TySubst::new())?;
        }
        self.record_call_environment_effects(span.clone(), &callable, &[], &[], args, kwargs)?;
        self.record_at(span.clone()).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::ElementInvocation(Box::new(
                mojito_checked::checked::CheckedElementInvocation {
                    getter,
                    callable,
                    target,
                    raises: error.clone(),
                    receiver_register_passable: !self.memory_only(receiver_ty),
                },
            )),
        ));
        if let Some(error) = error.filter(|ty| *ty != TyKind::Never) {
            self.require_call_error(span, "call through a raising callable element", error)?;
        }
        Ok(ret)
    }

    /// Check a call of `ty`, named `name`, with bracket `param_args`;
    /// `signature` lists the `ref` contracts over each of the callee's
    /// origin binders, for an origin the bracket binds.
    pub(super) fn infer_callable_ty(
        &self,
        span: &SourceSpan,
        name: &str,
        ty: Ty,
        (param_args, signature): (
            &[mojito_ast::ast::ParamArg],
            Option<&CallableOriginSignature>,
        ),
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<InferredCall, TypeError> {
        // The bracket binds the whole declared list first, and what it
        // bound is substituted into the signature; then the ordinary
        // arguments bind, and infer the compile-time parameters left open.
        if let Some(generic) = ty.as_generic_func() {
            let explicit = self.explicit_type_args(name, &generic.decls, param_args)?;
            let bound = self.bind_bracket_origins(ty.clone(), explicit.origins());
            return self.infer_generic_call(
                span,
                name,
                &bound,
                (param_args, explicit),
                args,
                kwargs,
            );
        }
        let callee = ty.clone();
        let simd = ty.is_simd();
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
        ) = match ty.into_kind() {
            TyKind::Param {
                callable_bound: Some(bound),
                ..
            } => {
                return self
                    .infer_callable_ty(span, name, bound, (param_args, signature), args, kwargs)
                    .map(|call| with_callee(call, callee));
            }
            TyKind::Struct(struct_name, arguments) if !simd => {
                let actual = Ty::new(TyKind::Struct(struct_name.clone(), arguments));
                let callable = self
                    .declared_callable_contract(&actual)
                    .ok_or_else(|| self.struct_not_callable(name, &struct_name, param_args))?;
                return self
                    .infer_callable_ty(span, name, callable, (param_args, signature), args, kwargs)
                    .map(|call| with_callee(call, callee));
            }
            // A non-generic function takes no compile-time parameters.
            TyKind::Func(func) => {
                let FuncTy {
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
                } = *func;

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
        let kw_collector = kw_variadic;
        if forwarded_element.is_some() && kw_collector.is_none() {
            return Err(TypeError::BadCall {
                func: name.to_string(),
                reason: "`**kwargs^` requires a callee with a `**kwargs` collector".to_string(),
            });
        }
        self.reject_forwarded_pack(name, args, variadic.as_ref())?;
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
        mojito_ast::call::bind_keyword_forward(&matched, forwarded_element.is_some())
            .map_err(|error| error.into_type_error(name))?;
        let (slots, overflow, kw_overflow) = (
            matched.slots,
            matched.positional_overflow,
            matched.keyword_overflow,
        );
        // A function type declares no origin binder: a callee over one is
        // a generator, whose call binds it in the one solve.
        let declared = params.clone();
        let mut score = 0;
        let mut existentials = mojito_types::types::TySubst::new();
        for (i, slot) in slots.iter().enumerate() {
            let arg = match slot {
                ArgSlot::Positional(p) => &args[*p],
                ArgSlot::Keyword(k) => &kwargs[*k].value,
                ArgSlot::Default => continue,
            };
            let mut arg_ty = self.infer_with_expected(arg, &params[i], true)?;
            reject_generic_function_value(name, &names[i], arg, &arg_ty, &params[i])?;
            let expected = self.existential_argument_ty(&params[i], &arg_ty);
            if expected != params[i] {
                arg_ty = self.infer_with_expected(arg, &expected, true)?;
            }
            bind_existential(&mut existentials, &params[i], &expected);
            if !self.record_implicit_conversion(arg, &arg_ty, &expected)? {
                return Err(argument_mismatch(name, &names[i], &params[i], &arg_ty));
            }
            // A deduced existential still ranks as the generic binder it is,
            // below an overload declaring the argument's own type.
            score += conversion_count(&arg_ty, &params[i]);
            // Only a `var`/`deinit` parameter *consumes* its argument (moving the
            // value in). `read` (the default), `mut`, and `ref` all **borrow** — no
            // copy — so passing a non-Copyable value to them is fine.
            if let Some(Some(ArgConvention::Var | ArgConvention::Deinit)) = conventions.get(i) {
                self.check_consuming_as(
                    arg,
                    &arg_ty,
                    &format!("argument '{}' to '{}'", names[i], name),
                    super::traits::ConsumeKind::Forward,
                )?;
            }
        }
        let collected = match &variadic {
            Some(element) => {
                let (collected_score, collected) =
                    self.check_collected_arguments(name, element, &overflow, args)?;
                score += collected_score;
                collected
            }
            None => Vec::new(),
        };
        self.check_owned_collector(name, &overflow, args)?;
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

        // Each existential is the anonymous binder its argument deduced, so a
        // result naming it (`type_of(it).IteratorOwnedType`) closes over the
        // argument's type.
        let ret = self.close_existentials(ret, &existentials);
        // Borrow check (mutable-XOR-shared), root-sensitive: within one call a
        // variable borrowed exclusively (`mut`/`ref`) or moved (`^`) may not be
        // borrowed again — mutably, shared, or moved.
        let (effective_conventions, return_ref, bool_bindings) = self
            .solve_call_origins_with_bool_bindings(
                &slots,
                &conventions,
                (
                    &ref_params,
                    self.bind_ref_sig_pack_origin(ref_return.as_deref(), &overflow, args)?
                        .as_ref(),
                    &|_| false,
                ),
                args,
                kwargs,
                true,
            )
            .map_err(|error| name_ref_origin_mismatch(error, name, &names))?;
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
                let ty = self.infer_with_expected(expression, &params[index], true)?;
                Ok(self.argument_is_independent_copy(convention, expression, &ty))
            })
            .collect::<Result<Vec<_>, TypeError>>()?;
        let collected = self.collected_arguments(name, collected);
        self.check_free_call_aliasing(
            &self.free_callee(name, &names, &declared, &params, Some(&collected)),
            &effective_conventions,
            &copied_reads,
            &slots,
            args,
            kwargs,
        )?;
        self.record_call_conventions(
            span,
            &slots,
            &effective_conventions,
            &self.declared_slots(&conventions, &ref_params, &declared, &params),
            self.gathered_arguments(name, &overflow),
        );
        self.record_argument_borrows(
            &slots,
            &effective_conventions,
            self.gathered_arguments(name, &overflow),
            args,
            kwargs,
            None,
        );

        let result = self.free_call_result(span, return_ref, ret)?;
        Ok((
            result,
            concrete_overload_rank(score, variadic.is_some() || has_kw_collector, &declared)
                + self
                    .argument_binding(
                        variadic.as_ref().map(|_| overflow.len()),
                        &slots,
                        &conventions,
                        &params,
                        args,
                        kwargs,
                    )
                    .rank(),
            error,
            bool_bindings,
            0,
            DeclaredAttributes::default(),
            callee,
        ))
    }

    /// Type a call to a generic function: solve its type parameters from the
    /// argument types, then check each argument coerces to the substituted
    /// parameter type and return the substituted result type.
    /// [`Self::infer_callable_ty`] of the generic function `generic`, with
    /// what its bracket `param_args` bound, `explicit`, already substituted
    /// into its signature.
    fn infer_generic_call(
        &self,
        span: &SourceSpan,
        name: &str,
        generic: &Ty,
        (param_args, explicit): (&[mojito_ast::ast::ParamArg], ExplicitTypes),
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<InferredCall, TypeError> {
        let Some(GenericFuncTy {
            decls: binders,
            func:
                func @ FuncTy {
                    names,
                    required,
                    variadic,
                    kw_variadic,
                    positional_only,
                    keyword_only,
                    raises: _,
                    error,
                    conventions,
                    ref_params,
                    ..
                },
        }) = generic.as_generic_func()
        else {
            return Err(TypeError::InvariantViolation(format!(
                "generic call inference received non-generic callee '{name}'"
            )));
        };
        let decls = &*ParamDecl::argument_view(binders);
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
        let forwarded_pack = self.forwarded_pack_argument(name, args, variadic.is_some())?;
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
        mojito_ast::call::bind_keyword_forward(&matched, forwarded_element.is_some())
            .map_err(|error| error.into_type_error(name))?;
        if let Some((position, _)) = &forwarded_pack {
            mojito_ast::call::bind_spread(&matched, *position)
                .map_err(|error| error.into_type_error(name))?;
        }
        let (slots, overflow, kw_overflow) = (
            matched.slots,
            matched.positional_overflow,
            matched.keyword_overflow,
        );
        let collector = || {
            self.collector_names
                .get(name)
                .map_or("args", String::as_str)
                .to_string()
        };
        let mut use_params = Vec::new();
        let mut use_names = Vec::new();
        let mut arg_tys = Vec::new();
        let mut arg_exprs = Vec::new();
        // What the one solve reads: a `ref` argument whose clause names an
        // `AddressSpace` binder solves as the reference it lends.
        let (mut solve_params, mut solve_actuals) = (Vec::new(), Vec::new());
        for (i, slot) in slots.iter().enumerate() {
            let arg = match slot {
                ArgSlot::Positional(p) => &args[*p],
                ArgSlot::Keyword(k) => &kwargs[*k].value,
                ArgSlot::Default => continue,
            };
            let parameter = &func.params[i];
            use_params.push(parameter.clone());
            use_names.push(names[i].clone());
            let actual = self.solving_argument_ty(arg, parameter, decls, &explicit.subst)?;
            let actual = self.callable_bound_argument(name, &names[i], arg, actual, parameter)?;
            let (solve_param, solve_actual) = self.reference_solve_pair(
                ref_params.get(i).and_then(Option::as_ref),
                parameter,
                &actual,
                arg,
            );
            solve_params.push(solve_param);
            solve_actuals.push(solve_actual);
            arg_tys.push(actual);
            arg_exprs.push(arg);
        }
        if let Some(elem) = variadic.as_ref() {
            for &p in &overflow {
                use_params.push(elem.clone());
                use_names.push(collector());
                let actual = match &forwarded_pack {
                    Some((position, forwarded)) if *position == p => self.bind_forwarded_pack(
                        name,
                        &args[p],
                        forwarded,
                        elem,
                        self.owned_collectors.get(name).copied(),
                    )?,
                    _ => self.solving_argument_ty(&args[p], elem, decls, &explicit.subst)?,
                };
                arg_tys.push(actual);
                arg_exprs.push(&args[p]);
            }
        }
        let mut keyword_actuals = Vec::new();
        if let Some(element) = kw_variadic.as_ref() {
            for &index in &kw_overflow {
                let actual = self.infer(&kwargs[index].value)?;
                use_params.push(element.clone());
                use_names.push(kwargs[index].name.clone());
                arg_tys.push(actual.clone());
                keyword_actuals.push((index, actual));
            }
            if let Some(actual) = &forwarded_element {
                use_params.push(element.clone());
                use_names.push("kwargs".to_string());
                arg_tys.push(actual.clone());
            }
        }
        // One solve binds the declared list: its origin binders from the
        // places the arguments lend, beside its type and value binders.
        let SolvedCall {
            bound:
                BoundParameters {
                    subst,
                    arguments: tyargs,
                    mut attributes,
                    ..
                },
            origins,
        } = self.resolve_call_params(
            name,
            binders,
            (param_args, &explicit),
            (
                &use_names,
                &[&solve_params[..], &use_params[solve_params.len()..]].concat(),
            ),
            &[&solve_actuals[..], &arg_tys[solve_actuals.len()..]].concat(),
        )?;
        let (use_params, params, variadic, ret, ref_return) =
            self.bound_generic_callee(func, origins.as_ref(), &use_params);
        let params = &params;
        let bracket = explicit
            .origins()
            .iter()
            .map(|bound| (bound.id, bound.origin.clone()))
            .collect();
        let declared = self.declared_call_arguments(binders, &tyargs, &bracket, origins.as_ref());
        let CallValues { values, open } = self.call_values((binders, &declared), decls, &tyargs);
        let resolve = |ty: &Ty| {
            let expanded = super::generics::expand_solved_packs(ty, decls, &tyargs);
            let substituted = self.resolve_assoc_ty(&substitute(&expanded, &subst));
            self.resolve_dependent_ty(&substituted, &values)
                .map(|resolved| self.defer_mutability_reads(&resolved, &open))
        };
        self.record_pack_element_conversions(decls, &tyargs, &use_params, &arg_tys, &arg_exprs)?;
        let (mut conversions, mut simd_erasures) = (0, 0);
        for ((aty, pty), expression) in arg_tys.iter().zip(&use_params).zip(arg_exprs) {
            if matches!(pty.kind(), TyKind::Param { binder, .. } if binder.name.starts_with('*')) {
                // Each pack element was checked independently against the pack's
                // bounds during inference; there is intentionally no single
                // substituted element type to coerce every argument into.
                conversions += pack_element_conversion_count(aty);
                continue;
            }
            simd_erasures += usize::from(
                matches!(pty.kind(), TyKind::Param { .. })
                    && mojito_types::types::simd_shape(aty).is_some(),
            );
            let expected = resolve(pty)?;
            // Re-inferred against the solved parameter: a display argument
            // constructs that parameter's collection, and a parameter that
            // resolves to a reference-bearing type only once explicit value
            // arguments are substituted (`Ts[index]` over a struct with a
            // `ref` field) takes the stored handle, not its referent.
            let actual = self.infer_with_expected(expression, &expected, true)?;
            if !self.record_implicit_conversion(expression, &actual, &expected)? {
                return Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: actual.to_string(),
                    context: format!("argument to '{name}'"),
                });
            }
            conversions += conversion_count(&actual, &expected);
        }
        if let Some(element) = kw_variadic.as_ref() {
            conversions += self.check_collected_keywords(
                name,
                &resolve(element)?,
                (keyword_actuals, forwarded_element),
                kwargs,
            )?;
        }
        for (i, slot) in slots.iter().enumerate() {
            if let Some(Some(ArgConvention::Var | ArgConvention::Deinit)) = conventions.get(i) {
                let arg = match slot {
                    ArgSlot::Positional(p) => &args[*p],
                    ArgSlot::Keyword(k) => &kwargs[*k].value,
                    ArgSlot::Default => continue,
                };
                let expected = resolve(&params[i])?;
                let ty = self.infer_with_expected(arg, &expected, true)?;
                self.check_consuming_as(
                    arg,
                    &ty,
                    &format!("argument '{}' to '{}'", names[i], name),
                    super::traits::ConsumeKind::Forward,
                )?;
            }
        }
        self.check_owned_collector(name, &overflow, args)?;
        let open = |id| binds_from_arguments(binders, &explicit, id);
        // The `ref` contracts at the solved binders: a clause's
        // `AddressSpace` binder closes to the space the call bound.
        let close = |ty: &Ty| resolve(ty).unwrap_or_else(|_| ty.clone());
        let ref_params = close_ref_sigs(ref_params, &close);
        let ref_return = ref_return
            .as_deref()
            .map(|signature| close_ref_space(signature.clone(), &close));
        let (effective_conventions, return_ref, bool_bindings) = self
            .solve_call_origins_with_bool_bindings(
                &slots,
                conventions,
                (
                    &ref_params,
                    self.bind_ref_sig_pack_origin(ref_return.as_ref(), &overflow, args)?
                        .as_ref(),
                    &open,
                ),
                args,
                kwargs,
                true,
            )
            .map_err(|error| name_ref_origin_mismatch(error, name, names))?;
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
                // Contextual, as the argument was just checked: a bare
                // inference would re-record a display as an `Array`.
                let ty = match resolve(&params[index]) {
                    Ok(expected) => self.infer_with_expected(expression, &expected, true)?,
                    Err(_) => self.infer(expression)?,
                };
                Ok(self.argument_is_independent_copy(convention, expression, &ty))
            })
            .collect::<Result<Vec<_>, TypeError>>()?;
        let bound: Vec<Ty> = params
            .iter()
            .map(|parameter| resolve(parameter).unwrap_or_else(|_| parameter.clone()))
            .collect();
        let collected =
            self.generic_collected_arguments(name, variadic.as_ref(), &overflow, args, resolve);
        self.check_free_call_aliasing(
            &self.free_callee(name, names, params, &bound, Some(&collected)),
            &effective_conventions,
            &copied_reads,
            &slots,
            args,
            kwargs,
        )?;
        let facts = self.declared_slots(conventions, &ref_params, params, &bound);
        let gathered = self.gathered_arguments(name, &overflow);
        self.record_call_conventions(span, &slots, &effective_conventions, &facts, gathered);
        self.record_argument_borrows(&slots, &effective_conventions, gathered, args, kwargs, None);
        let mut referent = resolve(&ret)?;
        if forwarded_pack.is_some() {
            // A result naming the callee's pack elements (`Us[i]`) closes
            // over the caller's own pack.
            referent = self.close_pack_elements(referent, &[(decls, &tyargs)]);
        }
        let result = self.free_call_result(span, return_ref, referent)?;
        let error = error.as_ref().map(resolve).transpose()?;
        self.call_bindings
            .borrow_mut()
            .insert(span.clone(), subst.clone());
        attributes.merge(folded_parameter_attributes(binders, param_args, &tyargs));
        // An overload candidate's write here is undone with its probe; the
        // selected candidate's re-run is the only one kept.
        let solved = (&binders[..], declared, &bool_bindings);
        self.record_generic_application(span, name, generic, (param_args, decls), &tyargs, solved)?;
        Ok((
            result,
            overload_rank(
                conversions,
                variadic.is_some() || kw_variadic.is_some(),
                decls.len(),
                true,
            ) + self
                .argument_binding(
                    variadic.as_ref().map(|_| overflow.len()),
                    &slots,
                    conventions,
                    params,
                    args,
                    kwargs,
                )
                .rank(),
            error,
            bool_bindings,
            simd_erasures,
            attributes,
            generic.clone(),
        ))
    }

    /// A free call's result: `referent`, read through the reference it
    /// `returned` by its `ref` result convention, which the call records.
    fn free_call_result(
        &self,
        span: &SourceSpan,
        returned: Option<mojito_types::origin::RefTy>,
        referent: Ty,
    ) -> Result<Ty, TypeError> {
        match returned {
            Some(reference) => self.record_reference_result(
                span,
                mojito_types::origin::RefTy {
                    referent,
                    ..reference
                },
            ),
            None => Ok(referent),
        }
    }

    /// The `where` clauses of `binders`' origin `mut=` binders, the last
    /// binders of their generator, at a call whose solved arguments over
    /// the whole declared list are `declared`
    /// ([`Self::declared_call_arguments`]), as upstream evaluates a
    /// generator's clause with every parameter bound. A `mut=` binder only
    /// a `ref` argument decides takes the mutability the call's origin
    /// solve bound it to (`ref_mutability`, by its
    /// [`SigMutability::BoolParam`](mojito_types::origin::SigMutability)
    /// slot). The argument view's own clauses were checked as the call
    /// solved it.
    fn validate_mutability_clauses(
        &self,
        name: &str,
        binders: &[ParamDecl],
        declared: &[TyArg],
        ref_mutability: &HashMap<usize, bool>,
    ) -> Result<(), TypeError> {
        if declared.len() != binders.len()
            || ParamDecl::mutability_binders(binders).all(|binder| binder.constraints().is_empty())
        {
            return Ok(());
        }
        let arguments: Vec<TyArg> = binders
            .iter()
            .zip(declared)
            .enumerate()
            .map(|(index, (binder, argument))| match argument {
                TyArg::Val(CtValue::Deferred(_)) if binder.is_origin_mutability_binder(binders) => {
                    let slot = binders[..index]
                        .iter()
                        .filter(|earlier| !earlier.is_pack_list())
                        .count();
                    ref_mutability.get(&slot).map_or_else(
                        || argument.clone(),
                        |&mutable| TyArg::Val(CtValue::Bool(mutable)),
                    )
                }
                _ => argument.clone(),
            })
            .collect();
        let environment = ConstraintEnvironment::declared(binders, &arguments);
        ParamDecl::mutability_binders(binders)
            .flat_map(ParamDecl::constraints)
            .try_for_each(|constraint| {
                self.validate_constraint_in_environment(name, constraint, &environment)
            })
    }

    /// Record the call at `span` of `generic`, named `name`, with bracket
    /// `param_args` and solved arguments `tyargs` over its argument view
    /// `decls`, once its `mut=` binders' clauses hold over its arguments
    /// over the whole declared list `binders`
    /// ([`Self::validate_mutability_clauses`]): the already-minted clone a
    /// closed inferred application retargets to by exact name instead of
    /// requesting a discovery round of its own (an earlier round minted it
    /// for another occurrence), or the application, with those arguments,
    /// `declared`, its solved arguments over the whole list
    /// ([`Self::declared_call_arguments`]).
    fn record_generic_application(
        &self,
        span: &SourceSpan,
        name: &str,
        generic: &Ty,
        (param_args, decls): (&[mojito_ast::ast::ParamArg], &[ParamDecl]),
        tyargs: &[TyArg],
        (binders, declared, ref_mutability): (&[ParamDecl], Vec<TyArg>, &HashMap<usize, bool>),
    ) -> Result<(), TypeError> {
        let Some(func) = generic.as_generic_func() else {
            return Ok(());
        };
        self.validate_mutability_clauses(name, binders, &declared, ref_mutability)?;
        if param_args.is_empty()
            && let Some(clone) = self.existing_def_clone(name, decls, tyargs)
        {
            self.clear_fact(span, |facts| facts.generic_instantiation.take());
            self.record_at(span.clone()).overload_target = Some(clone);
        } else {
            self.record_at(span.clone()).generic_instantiation =
                Some(mojito_checked::checked::GenericInstantiation {
                    callee: name.to_string(),
                    parameter_names: func.names.clone(),
                    parameter_types: func
                        .params
                        .iter()
                        .map(|ty| TypeKey::from_ty(ty).as_str().to_string())
                        .collect(),
                    variadic: mojito_symbol::symbol::VariadicKey::from_callable(generic),
                    arguments: declared,
                });
        }
        Ok(())
    }

    /// The already-minted clone of bound-generic `name` for a closed inferred
    /// application, when the program declares it: the specializer names a
    /// clone `mangle(name, values)` over the declaration-order type and value
    /// arguments (`def_request_values`), so an occurrence whose clone exists
    /// needs no discovery round. Anything that does not resolve to a declared
    /// concrete function keeps the request path.
    pub(super) fn existing_def_clone(
        &self,
        name: &str,
        decls: &[ParamDecl],
        arguments: &[TyArg],
    ) -> Option<String> {
        if decls.is_empty() || decls.len() != arguments.len() {
            return None;
        }
        let mut values = Vec::new();
        for (decl, argument) in ParamDecl::aligned_pairs(decls, arguments) {
            match (decl, argument) {
                (ParamDecl::Type { variadic: true, .. }, _) => return None,
                // A deferred callable-value slot is not part of the key; a
                // residual value names no clone at all (`mangle` refuses it).
                (_, TyArg::Val(CtValue::Deferred(_))) => continue,
                (ParamDecl::Type { .. }, TyArg::Ty(ty)) => {
                    values.push(CtValue::Type(ty.clone()));
                }
                (ParamDecl::Value { .. }, TyArg::Val(value)) => values.push(value.clone()),
                _ => return None,
            }
        }
        if values.is_empty() {
            return None;
        }
        let clone = mojito_symbol::symbol::mangle(name, &values).ok()?;
        matches!(self.lookup(&clone).map(Ty::kind), Some(TyKind::Func(_))).then_some(clone)
    }

    pub(super) fn forwarded_kwargs_element(
        &self,
        callee: &str,
        kwargs: &[mojito_ast::ast::KwArg],
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
        match found.kind() {
            struct_ty
                if let Some((name, args)) = struct_ty.as_non_pointer_struct()
                    && name == "StringDict" =>
            {
                match args.as_slice() {
                    [TyArg::Ty(element)] => Ok(Some(element.clone())),
                    _ => Err(TypeError::InvariantViolation(
                        "StringDict must carry one value type".to_string(),
                    )),
                }
            }
            other => Err(TypeError::TypeMismatch {
                expected: "StringDict[T]".to_string(),
                found: other.to_string(),
                context: format!("forwarded keyword arguments to '{callee}'"),
            }),
        }
    }

    /// How a candidate binds the call's arguments to its regular parameters,
    /// for the terms `ArgumentBinding` owns. `collector` is how many
    /// arguments the candidate's `*args` collector takes, or `None` when it
    /// has none: a candidate without a collector pays only the copies, so its
    /// parameter types are never asked whether they are trivially
    /// register-passable.
    pub(in crate::checker) fn argument_binding(
        &self,
        collector: Option<usize>,
        slots: &[ArgSlot],
        conventions: &[Option<ArgConvention>],
        params: &[Ty],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> ArgumentBinding {
        let mut binding = ArgumentBinding::new(collector);
        for (index, slot) in slots.iter().enumerate() {
            let argument = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            binding.bind(
                conventions.get(index).copied().flatten(),
                collector.is_some()
                    && params
                        .get(index)
                        .is_some_and(|param| self.is_trivial_register_passable(param)),
                argument,
            );
        }
        binding
    }

    /// [`Self::resolve_use_params`] for a call of `name`, over `decls`, its
    /// callee's whole declared list, as upstream's one solve binds it: the
    /// origin binders the bracket leaves open bind from the places the
    /// arguments lend the parameters naming them (`patterns`, named
    /// `names`, against `actuals`), and its type and value binders solve
    /// from the arguments' types with those origins bound. A `_` the
    /// bracket spells for an origin binder takes the origin it bound as its
    /// attribute. An argument the element bound of the callee's pack
    /// collector refuses is reported as such
    /// ([`Self::collector_element_failure`]), and each existential `Some[Trait]`
    /// pattern solves to its argument's deduced type ([`bind_existential`]).
    pub(super) fn resolve_call_params(
        &self,
        name: &str,
        decls: &[ParamDecl],
        bracket: (&[mojito_ast::ast::ParamArg], &ExplicitTypes),
        (names, patterns): (&[String], &[Ty]),
        actuals: &[Ty],
    ) -> Result<SolvedCall, TypeError> {
        let open: Vec<_> = super::origins::declared_origin_binders(decls)
            .filter(|id| !bracket.1.origins().iter().any(|bound| bound.id == *id))
            .collect();
        let places: Vec<(usize, &Ty)> = patterns
            .iter()
            .zip(actuals)
            .enumerate()
            .filter(|(_, (pattern, _))| super::origins::names_origin_binder(pattern))
            .map(|(index, (_, actual))| (index, actual))
            .collect();
        let origins = if open.is_empty() || places.is_empty() {
            None
        } else {
            Some(Self::bind_callee_origins(
                name,
                names,
                patterns,
                &places,
                super::origins::CalleeBinders::Declared(open),
            )?)
            .filter(|origins| !origins.is_empty())
        };
        if let Some(origins) = &origins {
            self.check_bracket_fixed_mutability(
                name,
                decls,
                bracket.1,
                (names, patterns),
                (&places, origins),
            )?;
        }
        let patterns = origins.as_ref().map_or_else(
            || patterns.to_vec(),
            |origins| origins.substitute_all(patterns),
        );
        let mut bound = self
            .resolve_explicit_use_params(name, decls, bracket, &patterns, actuals)
            .map_err(|failure| self.collector_element_failure(name, decls, failure))?;
        for (pattern, actual) in patterns.iter().zip(actuals) {
            let deduced = self.existential_argument_ty(pattern, actual);
            bind_existential(&mut bound.subst, pattern, &deduced);
        }
        bind_unbound_origin_attributes(
            name,
            &mut bound.attributes,
            (decls, bracket.0),
            origins.as_ref(),
        )?;
        Ok(SolvedCall { bound, origins })
    }

    /// Reject an argument binding an origin binder to an immutable origin
    /// where the call's bracket fixed that binder's `mut=` binder to
    /// `True`, as upstream binds the explicit parameters before the
    /// arguments: `f[t=MutUntrackedOrigin](q)` over `[m: Bool, //, o:
    /// Origin[mut=m], t: Origin[mut=m]](p: Pointer[Int, o])` (or
    /// `f[m=True](q)`) binds `m = True`, so an immutable `q` no longer
    /// converts to `Pointer[Int, o]`. `places` pairs each parameter index
    /// naming a binder with its actual, and `bindings` the binders they
    /// bound ([`super::origins::bracket_mutable_binders`]).
    fn check_bracket_fixed_mutability(
        &self,
        name: &str,
        decls: &[ParamDecl],
        explicit: &ExplicitTypes,
        (names, patterns): (&[String], &[Ty]),
        (places, bindings): (&[(usize, &Ty)], &super::origins::ConstructorOriginBindings),
    ) -> Result<(), TypeError> {
        use mojito_types::origin::{Mutability, Origin};
        let fixed = super::origins::bracket_mutable_binders(
            decls,
            explicit
                .mutabilities()
                .iter()
                .map(|bound| (&bound.id, &bound.value)),
            explicit.origins(),
        );
        let immutable: Vec<_> = fixed
            .into_iter()
            .filter(|id| {
                bindings.bound_mutability(*id, &|owner| self.owner_reference_mutability(owner))
                    == Some(Mutability::Immutable)
            })
            .collect();
        if immutable.is_empty() {
            return Ok(());
        }
        for &(index, actual) in places {
            let Some(pattern) = patterns.get(index) else {
                continue;
            };
            if super::origins::names_origin_binder_in(pattern, &|id| immutable.contains(&id)) {
                let parameter = names.get(index).map_or("?", String::as_str);
                // The parameter's type spells each origin binder by its
                // declared name (`Pointer[Int, o]`), as upstream prints it.
                let mut binders: Vec<_> = decls
                    .iter()
                    .filter_map(|decl| match decl {
                        ParamDecl::Origin { origin, .. } => Some((*origin, decl.name())),
                        _ => None,
                    })
                    .collect();
                binders.sort_by_key(|(id, _)| std::cmp::Reverse(id.0));
                let expected = binders
                    .iter()
                    .fold(pattern.to_string(), |text, (id, binder)| {
                        text.replace(&Origin::Param(*id).to_string(), binder)
                    });
                return Err(TypeError::BadCall {
                    func: name.to_string(),
                    reason: format!(
                        "value passed to '{parameter}' cannot be converted from '{actual}' to \
                         '{expected}'"
                    ),
                });
            }
        }
        Ok(())
    }

    /// Record each pack element's conversion to the element the call
    /// solved, when it solved the pack. Each element was checked against the
    /// pack's bounds during inference, and there is no single substituted
    /// element type to coerce every argument into; a literal element
    /// converts to its solved element, which the template's instance takes
    /// it as.
    fn record_pack_element_conversions(
        &self,
        decls: &[ParamDecl],
        tyargs: &[TyArg],
        use_params: &[Ty],
        arg_tys: &[Ty],
        arg_exprs: &[&Expr],
    ) -> Result<(), TypeError> {
        let solved = ParamDecl::aligned_pairs(decls, tyargs).find_map(|(decl, argument)| {
            match (decl, argument) {
                (ParamDecl::Type { variadic: true, .. }, TyArg::Val(CtValue::Tuple(elements))) => {
                    Some(elements)
                }
                _ => None,
            }
        });
        let Some(elements) = solved else {
            return Ok(());
        };
        let pack_arguments = use_params.iter().zip(arg_tys).zip(arg_exprs).filter(
            |((pty, _), _)| matches!(pty.kind(), TyKind::Param { binder, .. } if binder.name.starts_with('*')),
        );
        for (((_, found), expression), element) in pack_arguments.zip(elements) {
            if let CtValue::Type(element) = element {
                self.record_implicit_conversion(expression, found, element)?;
            }
        }
        Ok(())
    }

    /// Check each argument a `*args` collector gathers at `overflow` against
    /// its element type, returning the
    /// conversion score and each element's declared and found types for the
    /// exclusivity rule.
    fn check_collected_arguments<'a>(
        &self,
        name: &str,
        element: &Ty,
        overflow: &[usize],
        args: &'a [Expr],
    ) -> Result<(usize, CollectedElements<'a>), TypeError> {
        let arity_mismatch = |expected: usize| TypeError::ArityMismatch {
            name: name.to_string(),
            expected,
            got: overflow.len(),
        };
        let mut score = 0;
        let mut collected = Vec::new();
        for (pack_index, &position) in overflow.iter().enumerate() {
            let expected = match element.kind() {
                TyKind::RuntimePack(elements) => elements
                    .get(pack_index)
                    .ok_or_else(|| arity_mismatch(elements.len()))?,
                _ => element,
            };
            let argument = &args[position];
            let found = self.infer_with_expected(argument, expected, true)?;
            // A checker-minted pack clone materializes a literal element
            // as its nominal type (`"lit"` → `String`): the element
            // converts exactly as a regular argument does.
            if !self.record_implicit_conversion(argument, &found, expected)? {
                return Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: found.to_string(),
                    context: format!("variadic argument to '{name}'"),
                });
            }
            score += conversion_count(&found, expected);
            let declared = match element.kind() {
                TyKind::RuntimePack(elements) => &elements[pack_index],
                _ => element,
            };
            collected.push((argument, vec![declared.clone(), found]));
        }
        match element.kind() {
            TyKind::RuntimePack(elements) if elements.len() != overflow.len() => {
                Err(arity_mismatch(elements.len()))
            }
            _ => Ok((score, collected)),
        }
    }

    /// [`Self::check_consuming_collected`] for the free callee `name`.
    fn check_owned_collector(
        &self,
        name: &str,
        overflow: &[usize],
        args: &[Expr],
    ) -> Result<(), TypeError> {
        self.check_consuming_collected(
            self.owned_collectors.get(name) == Some(&true),
            overflow,
            args,
            &format!("'{name}'"),
        )
    }

    /// `__param_list_address[*values]()`: a `Pointer[E, ImmStaticOrigin]` to
    /// the first of the value pack's elements, laid out once per program.
    fn infer_param_list_address(
        &self,
        span: SourceSpan,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        let (argument, values) = match param_args {
            [argument] if args.is_empty() => self
                .raw_list_argument(argument)
                .map(|values| (argument, values)),
            _ => None,
        }
        .ok_or_else(|| {
            TypeError::Unsupported(format!(
                "'{PARAM_LIST_ADDRESS}' takes one raw parameter list and no arguments"
            ))
        })?;
        let attributes = DeclaredAttributes::of_builtin(
            PARAM_LIST_ADDRESS,
            VALUE_PACK_PARAMS,
            param_args,
            vec![bracket_attribute(
                argument,
                &TyArg::Val(CtValue::Expr(values.clone())),
            )],
        )?;
        self.record_declared_attributes(span.clone(), &attributes);
        let element = match values.meta() {
            mojito_types::param_expr::MetaTy::ParamList(element) => element.as_value().cloned(),
            _ => None,
        }
        .ok_or_else(|| {
            TypeError::InvariantViolation(format!(
                "the value pack `{values}` is not a parameter list of values"
            ))
        })?;
        self.record_at(span).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::ParamListAddress { values },
        ));
        Ok(Ty::pointer(
            element,
            mojito_types::origin::PointerOrigin::Static { mutable: false },
            mojito_types::types::AddressSpaceSlot::GENERIC,
        ))
    }

    /// `_mojito_float_text(value, out)`, `std.builtin._format_float`'s
    /// staged leaf: the shortest round-trip text of a `Float64` as bytes at
    /// `out`, returning how many, until R159's Dragonbox body writes it in
    /// Mojo.
    fn infer_float_text(&self, args: &[Expr]) -> Result<Ty, TypeError> {
        let tys = self.builtin_args("_mojito_float_text", 2, args)?;
        if !tys[0].is_float64() {
            return Err(TypeError::TypeMismatch {
                expected: "Float64".to_string(),
                found: tys[0].to_string(),
                context: "argument to '_mojito_float_text'".to_string(),
            });
        }
        if !tys[1].pointer_element().is_some_and(|element| {
            mojito_types::types::simd_slots(element).is_some_and(|(dtype, _)| {
                dtype == mojito_types::types::SimdDtype::Known(mojito_ast::ast::Dtype::UInt8)
            })
        }) {
            return Err(TypeError::TypeMismatch {
                expected: "Pointer[Byte]".to_string(),
                found: tys[1].to_string(),
                context: "argument to '_mojito_float_text'".to_string(),
            });
        }
        Ok(Ty::int())
    }

    /// Upstream's rejection of an argument the element bound of `name`'s
    /// pack collector refuses (`*values: *Ts` over `*Ts: Writable`): the
    /// call names the collector and the element's type, where any other
    /// `failure` stands.
    fn collector_element_failure(
        &self,
        name: &str,
        decls: &[ParamDecl],
        failure: TypeError,
    ) -> TypeError {
        let TypeError::TraitNotSatisfied {
            param,
            ty,
            trait_name,
            ..
        } = &failure
        else {
            return failure;
        };
        let Some(collector) = self.collector_names.get(name) else {
            return failure;
        };
        if ParamDecl::type_pack_list(decls, param).is_none() {
            return failure;
        }
        TypeError::BadCall {
            func: name.to_string(),
            reason: format!(
                "an element of '{collector}' with type '{ty}' does not conform to trait \
                 '{trait_name}'; either prove the conformance with 'conforms_to', or add \
                 conformance"
            ),
        }
    }

    /// The rejection of a call on a value of struct `struct_name` that
    /// declares no `__call__`. Value brackets over an indexable runtime
    /// value were re-dispatched as an element call before reaching here, so
    /// bracket arguments are type, named, or empty ones: neither a runtime
    /// subscript nor parameter application on a callable.
    fn struct_not_callable(
        &self,
        name: &str,
        struct_name: &str,
        param_args: &[mojito_ast::ast::ParamArg],
    ) -> TypeError {
        if !param_args.is_empty()
            && self
                .structs
                .get(struct_name)
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
            ty: struct_name.to_string(),
        }
    }

    /// Check the keywords a `**kwargs` collector gathers against its
    /// solved element `expected`, and a forwarded collector's element
    /// (`forwarded`); the conversions they take.
    fn check_collected_keywords(
        &self,
        name: &str,
        expected: &Ty,
        (keyword_actuals, forwarded): (Vec<(usize, Ty)>, Option<Ty>),
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<usize, TypeError> {
        let mut conversions = 0;
        for (index, actual) in keyword_actuals {
            let expression = &kwargs[index].value;
            if !self.record_implicit_conversion(expression, &actual, expected)? {
                return Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: actual.to_string(),
                    context: format!("keyword '{}' collected by '{}'", kwargs[index].name, name),
                });
            }
            self.check_consuming(
                expression,
                &actual,
                &format!("keyword '{}' collected by '{name}'", kwargs[index].name),
            )?;
            conversions += conversion_count(&actual, expected);
        }
        if let Some(actual) = forwarded
            && actual != *expected
        {
            return Err(TypeError::TypeMismatch {
                expected: format!("StringDict[{expected}]"),
                found: format!("StringDict[{actual}]"),
                context: format!("forwarded keyword arguments to '{name}'"),
            });
        }
        Ok(conversions)
    }

    /// The `(pattern, actual)` the call's one solve reads for `argument`:
    /// a `ref` argument whose clause names an `AddressSpace` binder
    /// (`ref[_, a] x`) solves as upstream's `!lit.ref<T, origin, a>`
    /// against the reference it lends, binding `a` to the space its place
    /// lives in; any other as its parameter type against its own.
    pub(super) fn reference_solve_pair(
        &self,
        signature: Option<&mojito_types::origin::RefSig>,
        parameter: &Ty,
        actual: &Ty,
        argument: &Expr,
    ) -> (Ty, Ty) {
        let Some(signature) = signature.filter(|signature| signature.address_space.is_symbolic())
        else {
            return (parameter.clone(), actual.clone());
        };
        let reference = |referent: &Ty, address_space| {
            Ty::from(TyKind::Ref(mojito_types::origin::RefTy {
                referent: referent.clone(),
                origin: mojito_types::origin::Origin::Static,
                mutability: mojito_types::origin::Mutability::Immutable,
                address_space,
            }))
        };
        (
            reference(parameter, signature.address_space.clone()),
            reference(actual, self.place_address_space(argument)),
        )
    }

    /// A generic callee's argument patterns `use_params`, parameters,
    /// `*args` element, result, and `ref` result contract with the origin
    /// binders its call's solve bound (`origins`) substituted, its type
    /// parameters still abstract.
    fn bound_generic_callee(
        &self,
        func: &FuncTy,
        origins: Option<&super::origins::ConstructorOriginBindings>,
        use_params: &[Ty],
    ) -> GenericCalleeOrigins {
        let bind = |tys: &[Ty]| {
            origins.map_or_else(|| tys.to_vec(), |origins| origins.substitute_all(tys))
        };
        let variadic = func.variadic.as_ref().map(|element| {
            origins.map_or_else(|| element.clone(), |origins| origins.substitute(element))
        });
        let (ret, ref_return) =
            self.bound_callee_result(origins, &func.ret, func.ref_return.as_deref());
        (
            bind(use_params),
            bind(&func.params),
            variadic,
            ret,
            ref_return,
        )
    }

    /// A callee's result and `ref` result contract with the origin binders
    /// its call bound substituted
    /// ([`ConstructorOriginBindings::substitute_ref_sig`]).
    fn bound_callee_result(
        &self,
        bindings: Option<&super::origins::ConstructorOriginBindings>,
        ret: &Ty,
        ref_return: Option<&mojito_types::origin::RefSig>,
    ) -> (Ty, Option<Box<mojito_types::origin::RefSig>>) {
        let Some(bindings) = bindings else {
            return (ret.clone(), ref_return.cloned().map(Box::new));
        };
        (
            bindings.substitute_result(ret, &|_, origin| self.bound_origin_mutability(origin)),
            ref_return.map(|reference| {
                Box::new(
                    bindings.substitute_ref_sig(reference, &|owner| {
                        self.owner_reference_mutability(owner)
                    }),
                )
            }),
        )
    }
}

/// The attributes of the bracket arguments `param_args` supplies to a
/// non-variadic value parameter of `decls` solved to a closed value
/// ([`CtValue::is_parameter_attribute`]), each that value keyed by its
/// declaration. `decls` is the whole declared list and `arguments` the
/// values its argument view's binders solved to. Mojo's call carries such
/// an argument as a parameter attribute alone, so it has no run-time form
/// at the call site.
pub(super) fn folded_parameter_attributes(
    decls: &[ParamDecl],
    param_args: &[mojito_ast::ast::ParamArg],
    arguments: &[TyArg],
) -> DeclaredAttributes {
    let mut folded = DeclaredAttributes::new(decls);
    for (position, (argument, index)) in param_args
        .iter()
        .zip(bracket_argument_slots(decls, param_args))
        .enumerate()
    {
        let Some(index) = index else {
            continue;
        };
        let Some(ParamDecl::Value {
            variadic: false, ..
        }) = decls.get(index)
        else {
            continue;
        };
        let Some(value @ TyArg::Val(closed)) =
            ParamDecl::argument_position(decls, index).and_then(|index| arguments.get(index))
        else {
            continue;
        };
        let is_value = match argument {
            mojito_ast::ast::ParamArg::Value(_) => true,
            mojito_ast::ast::ParamArg::Named { value, .. } => {
                matches!(&**value, mojito_ast::ast::ParamArg::Value(_))
            }
            mojito_ast::ast::ParamArg::Type(_) => false,
        };
        if is_value && closed.is_parameter_attribute() {
            folded.bind(index, position, Some(value.clone()));
        }
    }
    folded
}

/// What a call's one solve bound: its declared list's type and value
/// binders, and the origin binders its arguments' places bound.
pub(super) struct SolvedCall {
    pub(super) bound: BoundParameters,
    pub(super) origins: Option<super::origins::ConstructorOriginBindings>,
}

/// `signatures` with each address space closed by `close`
/// ([`close_ref_space`]).
pub(super) fn close_ref_sigs(
    signatures: &[Option<mojito_types::origin::RefSig>],
    close: &dyn Fn(&Ty) -> Ty,
) -> Vec<Option<mojito_types::origin::RefSig>> {
    signatures
        .iter()
        .map(|signature| {
            signature
                .clone()
                .map(|signature| close_ref_space(signature, close))
        })
        .collect()
}

/// `signature` with a symbolic address space closed as the call closes
/// its result type (`close`): at the call's bound binders, and a method's
/// at its receiver's struct arguments (`ref[Self.o, Self.a]`).
pub(super) fn close_ref_space(
    mut signature: mojito_types::origin::RefSig,
    close: &dyn Fn(&Ty) -> Ty,
) -> mojito_types::origin::RefSig {
    if !signature.address_space.is_symbolic() {
        return signature;
    }
    let closed = close(&Ty::from(TyKind::Ref(mojito_types::origin::RefTy {
        referent: Ty::none(),
        origin: mojito_types::origin::Origin::Static,
        mutability: mojito_types::origin::Mutability::Immutable,
        address_space: signature.address_space.clone(),
    })));
    if let TyKind::Ref(closed) = closed.kind() {
        signature.address_space = closed.address_space.clone();
    }
    signature
}

/// Rewrite the solver's slot-indexed `ref` argument mismatch, of an exact
/// origin or of an address space, into upstream's call diagnostic naming
/// the function and the parameter.
pub(super) fn name_ref_origin_mismatch(
    error: TypeError,
    func: &str,
    names: &[String],
) -> TypeError {
    match error {
        TypeError::RefOriginMismatch {
            slot,
            ty,
            expected,
            actual,
        } => TypeError::BadCall {
            func: func.to_string(),
            reason: format!(
                "value passed to '{}' cannot be converted from '{ty}' to ref '{ty}'\nnote: operand origin '{actual}' doesn't match expected origin '{expected}'",
                names.get(slot).map_or("argument", String::as_str)
            ),
        },
        TypeError::RefAddressSpaceMismatch {
            slot,
            ty,
            expected,
            actual,
        } => TypeError::BadCall {
            func: func.to_string(),
            reason: format!(
                "value passed to '{}' cannot be converted from '{ty}' to ref '{ty}'\nnote: operand address space '{actual}' doesn't match expected address space '{expected}'",
                names.get(slot).map_or("argument", String::as_str)
            ),
        },
        TypeError::RefMutabilityMismatch { slot, ty } => TypeError::BadCall {
            func: func.to_string(),
            reason: format!(
                "value passed to '{}' cannot be converted from '{ty}' to ref '{ty}'\nnote: operand mutability 'False' doesn't match expected mutability 'True'",
                names.get(slot).map_or("argument", String::as_str)
            ),
        },
        other => other,
    }
}

/// A generic callee's origin-bound argument patterns, parameters, `*args`
/// element, result, and `ref` result contract.
type GenericCalleeOrigins = (
    Vec<Ty>,
    Vec<Ty>,
    Option<Ty>,
    Ty,
    Option<Box<mojito_types::origin::RefSig>>,
);

/// The arguments a `*args` collector gathers, each with its declared and
/// found element types.
type CollectedElements<'a> = Vec<(&'a Expr, Vec<Ty>)>;

/// A free call whose callee is an overload set, as each candidate is
/// checked against it.
struct OverloadCall<'a> {
    span: &'a SourceSpan,
    name: &'a str,
    param_args: &'a [mojito_ast::ast::ParamArg],
    args: &'a [Expr],
    kwargs: &'a [mojito_ast::ast::KwArg],
}

/// What probing an overload set's candidates found: the call-compatible
/// ones, the failed availability clauses, the failed trait bounds, and an
/// argument that fails every candidate alike.
struct OverloadProbes {
    matches: Vec<CallableOverloadMatch>,
    availability_failures: Vec<Option<String>>,
    bound_failures: Vec<TypeError>,
    argument_failure: Option<TypeError>,
}

/// What checking one overload candidate contributes to the selection.
enum CandidateOutcome {
    Match(CallableOverloadMatch),
    /// Call-compatible, but an availability clause failed, with the
    /// clause's message when it declared one.
    Unavailable(Option<String>),
    /// Rejected only by a trait bound on a solved type argument.
    Bound(TypeError),
    /// Rejected by an argument itself, whatever the candidate.
    Argument(TypeError),
    Rejected,
}

const PARAM_LIST_ADDRESS: &str = "__param_list_address";

/// Whether origin binder `id` of the declared list `binders` is one a call
/// binds from its arguments: one its bracket (`explicit`) left open.
fn binds_from_arguments(
    binders: &[ParamDecl],
    explicit: &ExplicitTypes,
    id: mojito_types::origin::OriginParamId,
) -> bool {
    binders
        .iter()
        .any(|decl| matches!(decl, ParamDecl::Origin { origin, .. } if *origin == id))
        && !explicit.origins().iter().any(|bound| bound.id == id)
}

/// Reject a use of the declaration `signature` describes when it is
/// `@unavailable`, with its decorator's message.
fn reject_unavailable(signature: Option<&CallableOriginSignature>) -> Result<(), TypeError> {
    signature
        .and_then(|signature| signature.unavailable.clone())
        .map_or(Ok(()), |message| Err(TypeError::Unavailable(message)))
}

/// The declaration each bracket argument binds, one entry per argument, as
/// [`mojito_ast::call::match_parameter_slots`] binds them; every entry is
/// `None` when the list does not bind.
fn bracket_argument_slots(
    decls: &[ParamDecl],
    param_args: &[mojito_ast::ast::ParamArg],
) -> Vec<Option<usize>> {
    let keywords = mojito_ast::call::bracket_keywords(param_args);
    mojito_ast::call::match_parameter_slots(decls, &keywords).map_or_else(
        |_| vec![None; param_args.len()],
        |slots| slots.into_iter().map(Some).collect(),
    )
}

/// Current Mojo does not infer a generic function's specialization from the
/// parameter it is passed to, as it does not from a local annotation: only an
/// explicit specialization (`f[...]`) is a non-generic function value.
fn reject_generic_function_value(
    func: &str,
    parameter: &str,
    argument: &Expr,
    found: &Ty,
    expected: &Ty,
) -> Result<(), TypeError> {
    if matches!(found.kind(), TyKind::GenericFunc(_))
        && matches!(expected.kind(), TyKind::Func(_))
        && (matches!(argument.kind, ExprKind::Identifier(_))
            || argument.bracket_application().is_some())
    {
        return Err(TypeError::BadCall {
            func: func.to_string(),
            reason: format!(
                "value passed to '{parameter}' cannot be converted from the generic function \
                 '{found}' to '{expected}'; specialize it explicitly"
            ),
        });
    }
    Ok(())
}

/// The rejection of an argument of type `found` bound to the parameter
/// `param` of `callee`, declared `expected`: a type passed as a value is
/// upstream's metatype conversion failure.
fn argument_mismatch(callee: &str, param: &str, expected: &Ty, found: &Ty) -> TypeError {
    if mojito_types::types::is_metatype(found) {
        return TypeError::MetatypeArgument {
            callee: callee.to_string(),
            param: param.to_string(),
            found: found.to_string(),
            expected: expected.to_string(),
        };
    }
    TypeError::TypeMismatch {
        expected: expected.to_string(),
        found: found.to_string(),
        context: format!("argument '{param}' to '{callee}'"),
    }
}

/// `call` with `callee` as the callee it checked: a call through a
/// callable bound or a struct's `__call__` contract keeps the callee it
/// named.
fn with_callee(call: InferredCall, callee: Ty) -> InferredCall {
    let (ret, score, error, bool_bindings, simd_erasures, attributes, _) = call;
    (
        ret,
        score,
        error,
        bool_bindings,
        simd_erasures,
        attributes,
        callee,
    )
}

/// Record, as the attribute of each `_` a call's bracket `param_args`
/// spells for an origin binder of `decls`, the origin the call's arguments
/// bound that binder to (`origins`), as upstream's call carries every bound
/// parameter, and reject one no argument's place binds, as upstream fails
/// to infer it.
fn bind_unbound_origin_attributes(
    name: &str,
    attributes: &mut DeclaredAttributes,
    (decls, param_args): (&[ParamDecl], &[mojito_ast::ast::ParamArg]),
    origins: Option<&super::origins::ConstructorOriginBindings>,
) -> Result<(), TypeError> {
    let listed = match param_args.last() {
        Some(last)
            if mojito_ast::call::unbound_marker(last) == Some(mojito_ast::call::Unbound::Rest) =>
        {
            &param_args[..param_args.len() - 1]
        }
        _ => param_args,
    };
    let Ok(slots) =
        mojito_ast::call::match_parameter_slots(decls, &mojito_ast::call::bracket_keywords(listed))
    else {
        return Ok(());
    };
    for (position, (argument, slot)) in listed.iter().zip(slots).enumerate() {
        let argument = match argument {
            mojito_ast::ast::ParamArg::Named { value, .. } => &**value,
            positional => positional,
        };
        if mojito_ast::call::unbound_marker(argument) != Some(mojito_ast::call::Unbound::Hole) {
            continue;
        }
        let Some(ParamDecl::Origin {
            origin,
            name: binder,
            ..
        }) = decls.get(slot)
        else {
            continue;
        };
        let bound = origins
            .and_then(|origins| origins.tail(*origin))
            .ok_or_else(|| TypeError::BadCall {
                func: name.to_string(),
                reason: format!("failed to infer parameter '{binder}'"),
            })?;
        attributes.bind(slot, position, Some(TyArg::Origin(bound)));
    }
    Ok(())
}

//! Generic parameter classification, solving, substitution, and bound checks.

use super::*;
pub(super) use mojito_symbol::symbol::{
    materialized_instantiation_argument, specialized_method_values,
};
pub use mojito_types::types::{map_tyargs, substitute};

pub(super) fn unify(
    pattern: &Ty,
    actual: &Ty,
    subst: &mut HashMap<String, Ty>,
) -> Result<(), TypeError> {
    match pattern {
        Ty::Param { name, .. } => {
            let solved = default_literal(actual);
            match subst.get(name) {
                None => {
                    subst.insert(name.clone(), solved);
                }
                Some(existing) if *existing == solved => {}
                // A literal argument against an already-bound parameter
                // (`Pair[Float64](0.5, 1)`) materializes to the binding, as
                // it does against a concrete parameter type.
                Some(existing)
                    if matches!(actual, Ty::IntLiteral | Ty::FloatLiteral)
                        && coerces(actual, existing) => {}
                Some(existing) => {
                    return Err(TypeError::TypeMismatch {
                        expected: existing.to_string(),
                        found: solved.to_string(),
                        context: format!("type parameter '{}'", name),
                    });
                }
            }
            Ok(())
        }
        // Recurse into a parameterized struct pattern to solve nested type
        // parameters (`Pair[T]` against `Pair[Int]` solves `T = Int`). Value
        // arguments contribute no type solution. A structural mismatch is left
        // for the caller's coercion check to report.
        Ty::Struct(pn, pargs) => {
            if let Ty::Struct(an, aargs) = actual
                && pn == an
                && pargs.len() == aargs.len()
            {
                for (p, a) in pargs.iter().zip(aargs) {
                    if let (TyArg::Ty(p), TyArg::Ty(a)) = (p, a) {
                        unify(p, a, subst)?;
                    }
                }
            } else if is_span_struct_name(pn)
                && let Some(TyArg::Ty(element_pattern)) = pargs.first()
                && let Some(element) = list_element(actual)
            {
                // The checked analogue of `Span`'s `@implicit` constructor from
                // a `List`: a `List[X]` argument solves a `Span[T, _]`
                // parameter's element (`join[T](elems: Span[T, _])` called with
                // a `List[String]`); the argument then converts as a call
                // argument does.
                unify(element_pattern, element, subst)?;
            }
            Ok(())
        }
        Ty::Variant(pattern_alternatives) => {
            if let Ty::Variant(actual_alternatives) = actual
                && pattern_alternatives.len() == actual_alternatives.len()
            {
                for (pattern, actual) in pattern_alternatives.iter().zip(actual_alternatives) {
                    unify(pattern, actual, subst)?;
                }
            }
            Ok(())
        }
        Ty::Func {
            environment: pattern_environment,
            params: pattern_params,
            ret: pattern_ret,
            error: pattern_error,
            ..
        } => {
            if let Ty::Func {
                environment: actual_environment,
                params: actual_params,
                ret: actual_ret,
                error: actual_error,
                ..
            } = actual
            {
                if !callable_environment_coerces(actual_environment, pattern_environment) {
                    return Ok(());
                }
                for (pattern, actual) in pattern_params.iter().zip(actual_params) {
                    unify(pattern, actual, subst)?;
                }
                unify(pattern_ret, actual_ret, subst)?;
                if let (Some(pattern), Some(actual)) = (pattern_error, actual_error) {
                    unify(pattern, actual, subst)?;
                } else if let Some(pattern) = pattern_error {
                    unify(pattern, &Ty::Never, subst)?;
                }
            }
            Ok(())
        }
        // A reference pattern solves through its referent; the actual may be
        // the bare referent type (reference reads type as the referent).
        Ty::Ref(pattern_reference) => match actual {
            Ty::Ref(actual_reference) => unify(
                &pattern_reference.referent,
                &actual_reference.referent,
                subst,
            ),
            _ => unify(&pattern_reference.referent, actual, subst),
        },
        // A non-parameter pattern contributes no solution; coercion is checked
        // separately by the caller.
        _ => Ok(()),
    }
}

/// Substitute a member template type at a receiver's type arguments: type
/// parameters via [`substitute`], value parameters via the receiver's
/// `TyArg::Val` bindings — `other: Self` on an `Array[Int, 3]` receiver
/// becomes `Array[Int, 3]`, not `Array[Int, length]`.
pub(super) fn substitute_at(ty: &Ty, decls: &[ParamDecl], targs: &[TyArg]) -> Ty {
    substitute_assoc(
        ty,
        &AssocBindings {
            types: struct_subst(decls, targs),
            values: solved_value_bindings(decls, targs),
            origins: HashMap::new(),
        },
    )
}

/// The value-parameter bindings a resolved application implies: each
/// `ParamDecl::Value` paired with its solved `TyArg::Val` argument.
pub(super) fn solved_value_bindings(
    decls: &[ParamDecl],
    tyargs: &[TyArg],
) -> HashMap<String, CtValue> {
    decls
        .iter()
        .zip(tyargs)
        .filter_map(|(decl, argument)| match (decl, argument) {
            (ParamDecl::Value { name, .. }, TyArg::Val(value)) => {
                Some((name.clone(), value.clone()))
            }
            _ => None,
        })
        .collect()
}

/// Solve value parameters from a pattern/actual type pair, the value-argument
/// counterpart of [`unify`]: a pattern's symbolic `TyArg::Val(CtValue::Param)`
/// binds to the actual's value argument in the same slot (`Array[T, length]`
/// against `Array[Int, 3]` solves `length = 3`). First solution wins; a
/// structural mismatch contributes nothing, exactly like `unify`.
pub(super) fn solve_value_args(pattern: &Ty, actual: &Ty, out: &mut HashMap<String, CtValue>) {
    match (pattern, actual) {
        (Ty::Struct(pn, pargs), Ty::Struct(an, aargs))
            if pn == an && pargs.len() == aargs.len() =>
        {
            for (p, a) in pargs.iter().zip(aargs) {
                match (p, a) {
                    (TyArg::Val(CtValue::Param(name)), TyArg::Val(value)) => {
                        out.entry(name.clone()).or_insert_with(|| value.clone());
                    }
                    (TyArg::Ty(p), TyArg::Ty(a)) => solve_value_args(p, a, out),
                    _ => {}
                }
            }
        }
        (Ty::Ref(pattern_reference), Ty::Ref(actual_reference)) => {
            solve_value_args(&pattern_reference.referent, &actual_reference.referent, out)
        }
        (Ty::Ref(pattern_reference), _) => {
            solve_value_args(&pattern_reference.referent, actual, out)
        }
        _ => {}
    }
}

/// Replace every `Ty::SelfType` in `ty` with `replacement` (the conforming
/// struct type, or a bounded `T`). Recurses into struct/function types.
pub(super) fn substitute_self(ty: &Ty, replacement: &Ty) -> Ty {
    match ty {
        Ty::SelfType => replacement.clone(),
        Ty::Param {
            name,
            bounds,
            callable_bound,
        } => Ty::Param {
            name: name.clone(),
            bounds: bounds.clone(),
            callable_bound: callable_bound
                .as_ref()
                .map(|bound| Box::new(substitute_self(bound, replacement))),
        },
        Ty::Struct(name, args) => Ty::Struct(
            name.clone(),
            map_tyargs(args, |t| substitute_self(t, replacement)),
        ),
        Ty::Dependent(mojito_types::types::DependentType::Indexed { elements, index }) => {
            Ty::Dependent(mojito_types::types::DependentType::Indexed {
                elements: elements
                    .iter()
                    .map(|ty| substitute_self(ty, replacement))
                    .collect(),
                index: index.clone(),
            })
        }
        Ty::ComptimeList(elem) => Ty::ComptimeList(Box::new(substitute_self(elem, replacement))),
        Ty::Tuple(elems) => Ty::Tuple(
            elems
                .iter()
                .map(|t| substitute_self(t, replacement))
                .collect(),
        ),
        Ty::RuntimePack(elems) => Ty::RuntimePack(
            elems
                .iter()
                .map(|t| substitute_self(t, replacement))
                .collect(),
        ),
        Ty::VariadicPack(element) => {
            Ty::VariadicPack(Box::new(substitute_self(element, replacement)))
        }
        Ty::Variant(alternatives) => Ty::Variant(
            alternatives
                .iter()
                .map(|ty| substitute_self(ty, replacement))
                .collect(),
        ),
        Ty::Pointer { element, origin } => Ty::Pointer {
            element: Box::new(substitute_self(element, replacement)),
            origin: origin.clone(),
        },
        Ty::Assoc { base, name, args } => Ty::Assoc {
            base: Box::new(substitute_self(base, replacement)),
            name: name.clone(),
            args: map_tyargs(args, |t| substitute_self(t, replacement)),
        },
        Ty::Func {
            environment,
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
            transfers,
        } => Ty::Func {
            environment: environment.clone(),
            params: params
                .iter()
                .map(|p| substitute_self(p, replacement))
                .collect(),
            names: names.clone(),
            ret: Box::new(substitute_self(ret, replacement)),
            required: required.clone(),
            variadic: variadic
                .as_ref()
                .map(|v| Box::new(substitute_self(v, replacement))),
            kw_variadic: kw_variadic
                .as_ref()
                .map(|v| Box::new(substitute_self(v, replacement))),
            positional_only: *positional_only,
            keyword_only: *keyword_only,
            raises: *raises,
            error: error
                .as_ref()
                .map(|error| Box::new(substitute_self(error, replacement))),
            conventions: conventions.clone(),
            ref_params: ref_params.clone(),
            ref_return: ref_return.clone(),
            transfers: transfers.clone(),
        },
        Ty::Overload(candidates) => Ty::Overload(
            candidates
                .iter()
                .map(|candidate| substitute_self(candidate, replacement))
                .collect(),
        ),
        _ => ty.clone(),
    }
}

/// The bindings concrete resolution of a parameterized associated type
/// substitutes into its symbolic template: type parameters by name (the
/// enclosing struct's and the member's), value parameters by name, and origin
/// parameters by `OriginParamId`.
pub(super) struct AssocBindings {
    pub types: HashMap<String, Ty>,
    pub values: HashMap<String, CtValue>,
    pub origins: HashMap<u32, mojito_types::origin::Origin>,
}

/// Substitute a parameterized associated type's template with concrete
/// arguments. Types are substituted first with the ordinary type substitution;
/// a second pass then replaces symbolic value parameters (`CtValue::Param`) and
/// origin parameters (`Origin::Param`), which the type-only pass carries through.
pub(super) fn substitute_assoc(ty: &Ty, bindings: &AssocBindings) -> Ty {
    let typed = substitute(ty, &bindings.types);
    substitute_values_and_origins(&typed, &bindings.values, &bindings.origins)
}

fn substitute_values_and_origins(
    ty: &Ty,
    values: &HashMap<String, CtValue>,
    origins: &HashMap<u32, mojito_types::origin::Origin>,
) -> Ty {
    let recur = |t: &Ty| substitute_values_and_origins(t, values, origins);
    let map_args = |args: &[TyArg]| -> Vec<TyArg> {
        args.iter()
            .map(|argument| match argument {
                TyArg::Ty(inner) => TyArg::Ty(recur(inner)),
                TyArg::Val(CtValue::Param(name)) => TyArg::Val(
                    values
                        .get(name)
                        .cloned()
                        .unwrap_or(CtValue::Param(name.clone())),
                ),
                TyArg::Val(value) => TyArg::Val(value.clone()),
                TyArg::Origin(origin) => TyArg::Origin(substitute_origin(origin, origins)),
            })
            .collect()
    };
    match ty {
        Ty::Struct(name, args) => Ty::Struct(name.clone(), map_args(args)),
        Ty::Assoc { base, name, args } => Ty::Assoc {
            base: Box::new(recur(base)),
            name: name.clone(),
            args: map_args(args),
        },
        Ty::Pointer { element, origin } => Ty::Pointer {
            element: Box::new(recur(element)),
            origin: origin.clone(),
        },
        Ty::Ref(reference) => {
            let mut reference = reference.clone();
            reference.referent = Box::new(recur(&reference.referent));
            reference.origin = substitute_origin(&reference.origin, origins);
            Ty::Ref(reference)
        }
        Ty::Tuple(elements) => Ty::Tuple(elements.iter().map(recur).collect()),
        Ty::RuntimePack(elements) => Ty::RuntimePack(elements.iter().map(recur).collect()),
        Ty::VariadicPack(element) => Ty::VariadicPack(Box::new(recur(element))),
        Ty::Variant(alternatives) => Ty::Variant(alternatives.iter().map(recur).collect()),
        Ty::ComptimeList(element) => Ty::ComptimeList(Box::new(recur(element))),
        Ty::Dependent(mojito_types::types::DependentType::Indexed { elements, index }) => {
            Ty::Dependent(mojito_types::types::DependentType::Indexed {
                elements: elements.iter().map(recur).collect(),
                index: index.clone(),
            })
        }
        other => other.clone(),
    }
}

fn substitute_origin(
    origin: &mojito_types::origin::Origin,
    origins: &HashMap<u32, mojito_types::origin::Origin>,
) -> mojito_types::origin::Origin {
    use mojito_types::origin::Origin;
    match origin {
        Origin::Param(id) => origins
            .get(&id.0)
            .cloned()
            .unwrap_or_else(|| origin.clone()),
        Origin::Union(members) => Origin::Union(
            members
                .iter()
                .map(|m| substitute_origin(m, origins))
                .collect(),
        ),
        _ => origin.clone(),
    }
}

/// Callable specialization and method-generic instantiation moved from `checker.rs`.
impl Checker {
    /// Split the source parameter list at an explicit specialization site.
    /// Ordinary arguments are rewritten as named arguments before being handed
    /// to the generic binder; this preserves their source slot even when an
    /// erased or infer-only semantic parameter precedes them.
    pub(super) fn split_callable_specialization(
        &self,
        name: &str,
        arguments: &[mojito_ast::ast::ParamArg],
        signature: &CallableOriginSignature,
    ) -> Result<SplitCallableSpecialization, TypeError> {
        use mojito_ast::ast::ParamArg;

        if signature.origins.is_empty() {
            return Ok((arguments.to_vec(), Vec::new()));
        }
        let mut supplied = vec![false; signature.source.len()];
        let mut origins = vec![None; signature.origins.len()];
        let mut ordinary = Vec::new();
        let mut next_positional = 0;
        for argument in arguments {
            let (index, value) = match argument {
                ParamArg::Named {
                    name: argument_name,
                    value,
                } => {
                    let index = signature
                        .source
                        .iter()
                        .position(|parameter| parameter.name == *argument_name)
                        .ok_or_else(|| TypeError::BadCall {
                            func: name.to_string(),
                            reason: format!("unknown compile-time parameter '{argument_name}'"),
                        })?;
                    (index, (**value).clone())
                }
                other => {
                    while next_positional < signature.source.len()
                        && (signature.source[next_positional].infer_only
                            || supplied[next_positional])
                    {
                        next_positional += 1;
                    }
                    if next_positional == signature.source.len() {
                        return Err(TypeError::WrongTypeArgCount {
                            name: name.to_string(),
                            expected: signature
                                .source
                                .iter()
                                .filter(|parameter| !parameter.infer_only)
                                .count(),
                            got: arguments.len(),
                        });
                    }
                    let index = next_positional;
                    next_positional += 1;
                    (index, other.clone())
                }
            };
            let parameter = &signature.source[index];
            if parameter.infer_only {
                return Err(TypeError::Unsupported(format!(
                    "infer-only parameter '{}' cannot be supplied explicitly",
                    parameter.name
                )));
            }
            if supplied[index] {
                return Err(TypeError::BadCall {
                    func: name.to_string(),
                    reason: format!("parameter '{}' was supplied twice", parameter.name),
                });
            }
            supplied[index] = true;
            if let Some(origin_index) = parameter.origin {
                if let ParamArg::Value(expression) = &value {
                    self.operation_adjustments.borrow_mut().insert(
                        expression.source_span(),
                        mojito_checked::checked::SemanticAdjustment::EraseCompileTimeArgument,
                    );
                }
                origins[origin_index] = Some(self.explicit_origin_argument(&value)?);
            } else if parameter.ordinary {
                ordinary.push(ParamArg::Named {
                    name: parameter.name.trim_start_matches('*').to_string(),
                    value: Box::new(value),
                });
            } else {
                return Err(TypeError::Unsupported(format!(
                    "semantic parameter '{}' is inferred and cannot be supplied explicitly",
                    parameter.name
                )));
            }
        }

        let bindings = signature
            .origins
            .iter()
            .zip(origins)
            .filter_map(|(parameter, origin)| {
                origin.map(|origin| (parameter.slots.clone(), origin))
            })
            .collect::<Vec<_>>();
        Ok((ordinary, bindings))
    }

    pub(super) fn bind_callable_origins(
        &self,
        mut callable: Ty,
        bindings: &[(Vec<usize>, mojito_types::origin::Origin)],
    ) -> Ty {
        let (ref_params, ref_return) = match &mut callable {
            Ty::Func {
                ref_params,
                ref_return,
                ..
            }
            | Ty::GenericFunc {
                ref_params,
                ref_return,
                ..
            } => (ref_params, ref_return),
            _ => return callable,
        };
        for signature in ref_params.iter_mut().flatten() {
            signature.origin = bind_sig_origin(&signature.origin, bindings);
        }
        if let Some(signature) = ref_return {
            signature.origin = bind_sig_origin(&signature.origin, bindings);
        }
        callable
    }

    pub(super) fn prepare_callable_specialization(
        &self,
        name: &str,
        arguments: &[mojito_ast::ast::ParamArg],
        callable: Ty,
        signature: Option<&CallableOriginSignature>,
    ) -> Result<(Ty, Vec<mojito_ast::ast::ParamArg>), TypeError> {
        let Some(signature) = signature else {
            return Ok((callable, arguments.to_vec()));
        };
        let (ordinary, bindings) =
            self.split_callable_specialization(name, arguments, signature)?;
        Ok((self.bind_callable_origins(callable, &bindings), ordinary))
    }

    /// Materialize the monomorphic checked view of an explicitly specialized
    /// generic function value. Generic execution remains type-erased; only its
    /// callable contract is instantiated here.
    pub(super) fn instantiate_generic_callable_value(
        &self,
        name: &str,
        callable: Ty,
        arguments: &[mojito_ast::ast::ParamArg],
    ) -> Result<(Ty, Vec<TyArg>), TypeError> {
        let Ty::GenericFunc {
            environment,
            decls,
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
            transfers,
        } = callable
        else {
            return Ok((callable, Vec::new()));
        };
        let (subst, tyargs) = self.resolve_use_params(name, &decls, arguments, &[], &[])?;
        let values = Self::value_argument_environment(&decls, &tyargs);
        let resolve = |ty: &Ty| {
            let substituted = self.resolve_assoc_ty(&substitute(ty, &subst));
            self.resolve_dependent_ty(&substituted, &values)
        };
        let contract = Ty::Func {
            environment,
            params: params.iter().map(resolve).collect::<Result<Vec<_>, _>>()?,
            names,
            ret: Box::new(resolve(&ret)?),
            required,
            variadic: variadic
                .as_ref()
                .map(|parameter| resolve(parameter).map(Box::new))
                .transpose()?,
            kw_variadic: kw_variadic
                .as_ref()
                .map(|parameter| resolve(parameter).map(Box::new))
                .transpose()?,
            positional_only,
            keyword_only,
            raises,
            error: error
                .as_ref()
                .map(|error| resolve(error).map(Box::new))
                .transpose()?,
            conventions,
            ref_params,
            ref_return,
            transfers,
        };
        Ok((contract, tyargs))
    }

    pub(super) fn specialize_callable_value_candidate(
        &self,
        name: &str,
        arguments: &[mojito_ast::ast::ParamArg],
        callable: Ty,
        signature: Option<&CallableOriginSignature>,
    ) -> Result<Ty, TypeError> {
        let (callable, ordinary) =
            self.prepare_callable_specialization(name, arguments, callable, signature)?;
        // The pinned compiler accepts explicit Origin specialization of a
        // stateless nested function as a value but rejects materializing one
        // that has a capture environment.
        if !arguments.is_empty()
            && let Ty::Func { environment, .. } | Ty::GenericFunc { environment, .. } = &callable
            && matches!(
                environment,
                mojito_types::origin::CallableEnvironment::Capturing(_)
            )
        {
            return Err(TypeError::Unsupported(format!(
                "cannot materialize an explicit Origin specialization of '{name}': it has a \
                 capture environment; call it directly or use a capture-free function"
            )));
        }
        match callable {
            callable @ Ty::GenericFunc { .. } => self
                .instantiate_generic_callable_value(name, callable, &ordinary)
                .map(|(contract, _)| contract),
            callable @ Ty::Func { .. } if ordinary.is_empty() => Ok(callable),
            Ty::Func { .. } => Err(TypeError::WrongTypeArgCount {
                name: name.to_string(),
                expected: 0,
                got: ordinary.len(),
            }),
            other => Err(TypeError::NotCallable {
                name: name.to_string(),
                ty: other.to_string(),
            }),
        }
    }

    pub(super) fn infer_specialized_callable_value(
        &self,
        span: SourceSpan,
        name: &str,
        arguments: &[mojito_ast::ast::ParamArg],
        expected: Option<&Ty>,
        record: bool,
    ) -> Result<Option<Ty>, TypeError> {
        let Some(callable) = self.lookup(name).cloned() else {
            return Ok(None);
        };
        if !matches!(
            callable,
            Ty::Func { .. } | Ty::GenericFunc { .. } | Ty::Overload(_)
        ) {
            return Ok(None);
        }
        self.check_capture_access(name, false)?;
        if record && let Some(owner) = self.lookup_owner(name) {
            self.expression_bindings
                .borrow_mut()
                .insert(span.clone(), owner);
        }
        let signatures = self.lookup_callable_origins(name).unwrap_or_default();
        let (selected, target) = match callable {
            Ty::Overload(candidates) => {
                let expected = expected.ok_or_else(|| TypeError::BadCall {
                    func: name.to_string(),
                    reason: "an overloaded function value requires a contextual callable type"
                        .to_string(),
                })?;
                let mut matches = candidates
                    .iter()
                    .enumerate()
                    .filter_map(|(index, candidate)| {
                        let specialized = self
                            .specialize_callable_value_candidate(
                                name,
                                arguments,
                                candidate.clone(),
                                signatures.get(index),
                            )
                            .ok()?;
                        self.value_coerces(&specialized, expected)
                            .then(|| {
                                callable_lowered_name(name, candidate)
                                    .map(|target| (specialized, target))
                            })
                            .flatten()
                    })
                    .collect::<Vec<_>>();
                match matches.len() {
                    0 => {
                        return Err(TypeError::TypeMismatch {
                            expected: expected.to_string(),
                            found: format!("specialization of overload({name})"),
                            context: "overloaded callable value".to_string(),
                        });
                    }
                    1 => matches.pop().expect("one callable-value candidate"),
                    _ => {
                        return Err(TypeError::BadCall {
                            func: name.to_string(),
                            reason: format!(
                                "multiple specialized overloads fit expected type '{expected}'"
                            ),
                        });
                    }
                }
            }
            candidate => {
                let specialized = self.specialize_callable_value_candidate(
                    name,
                    arguments,
                    candidate,
                    signatures.first(),
                )?;
                (specialized, name.to_string())
            }
        };
        if record {
            self.overload_targets
                .borrow_mut()
                .insert(span.clone(), target);
            self.expression_types
                .borrow_mut()
                .insert(span.clone(), selected.clone());
        }
        Ok(Some(selected))
    }

    /// The name of the elaborator-minted clone of `method` for this
    /// instantiation (`isa$y3:Int`), when the receiver struct already
    /// declares it. The value list must agree with the elaborator's
    /// `method_request_values`: type arguments and value arguments in
    /// declaration order; callable-bounded parameters stay symbolic on the
    /// clone and contribute nothing; packs and symbolic placeholders make the
    /// call unspecializable.
    pub(super) fn specialized_method_clone(
        &self,
        owner: &str,
        method: &str,
        decls: &[ParamDecl],
        arguments: &[TyArg],
    ) -> Option<String> {
        let values = specialized_method_values(decls, arguments)?;
        // Nothing baked (only callable-bounded parameters) mangles to the
        // method's own name: there is no clone, and retargeting would loop.
        if values.is_empty() {
            return None;
        }
        let name = mojito_symbol::symbol::mangle(method, &values);
        self.structs
            .get(owner)
            .is_some_and(|info| info.methods.contains_key(&name))
            .then_some(name)
    }

    /// Record a generic-struct application reached as a constructor target or
    /// method-call receiver, for per-instantiation method-clone discovery.
    pub(super) fn record_struct_instantiation(
        &self,
        template: &str,
        arguments: &[TyArg],
        source: Option<&str>,
    ) {
        // Unstamped bundled code is checked for every program, and a
        // synthesized node has no source at all: neither is user-reachable
        // code, so instances seen only there keep the erased path.
        if source.is_none() || super::overload_support::is_bundled_module_source(source) {
            return;
        }
        // Only a struct whose parameters are all plain type parameters gets
        // clones (the elaborator's `instance_template`); value parameters,
        // callable-bounded parameters, and origin binders keep the erased
        // path, so their applications are not requests.
        let bakeable = self.structs.get(template).is_some_and(|info| {
            !info.decls.is_empty()
                && info.decls.iter().all(|decl| {
                    matches!(
                        decl,
                        ParamDecl::Type {
                            variadic: false,
                            callable_bound: None,
                            ..
                        }
                    )
                })
                && info.decls.len() == arguments.len()
                && !info.source_params.iter().any(|parameter| {
                    matches!(parameter.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet")
                        || parameter.is_origin_mutability_binder(&info.source_params)
                })
        });
        // A `StringLiteral` argument names no clone (`instance_method_clone_name`):
        // the instance keeps the erased path, so there is nothing to mint.
        if !bakeable
            || !arguments.iter().all(|argument| {
                matches!(argument, TyArg::Ty(ty)
                    if !mojito_types::types::contains_string_literal(ty))
            })
        {
            return;
        }
        let arguments: Vec<TyArg> = arguments
            .iter()
            .map(materialized_instantiation_argument)
            .collect();
        let mut recorded = self.struct_instantiations.borrow_mut();
        if !recorded
            .iter()
            .any(|existing| existing.template == template && existing.arguments == arguments)
        {
            recorded.push(mojito_checked::checked::StructInstantiation {
                template: template.to_string(),
                arguments,
            });
        }
    }

    /// The per-instantiation clone of `method` on the struct instance
    /// `owner[arguments]` (`get$y3:Int`), once the elaborator has appended it
    /// to the template's method list. The value list agrees with the
    /// elaborator's `method_request_values` over the struct's parameters.
    pub(super) fn instance_method_clone(
        &self,
        owner: &str,
        method: &str,
        arguments: &[TyArg],
    ) -> Option<String> {
        let info = self.structs.get(owner)?;
        let name =
            mojito_symbol::symbol::instance_method_clone_name(method, &info.decls, arguments)?;
        info.methods.contains_key(&name).then_some(name)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn instantiate_method_generics(
        &self,
        name: &str,
        signature: &MethodSig,
        params: &[Ty],
        variadic: Option<&Ty>,
        kw_variadic: Option<&Ty>,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<MethodInstantiation, TypeError> {
        if signature.decls.is_empty() {
            if !param_args.is_empty() {
                return Err(TypeError::WrongTypeArgCount {
                    name: name.to_string(),
                    expected: 0,
                    got: param_args.len(),
                });
            }
            return Ok((
                params.to_vec(),
                variadic.cloned(),
                kw_variadic.cloned(),
                HashMap::new(),
                HashMap::new(),
            ));
        }
        let forwarded_element = self.forwarded_kwargs_element(name, kwargs)?;
        if forwarded_element.is_some() && kw_variadic.is_none() {
            return Err(TypeError::BadCall {
                func: name.to_string(),
                reason: "`**kwargs^` requires a callee with a `**kwargs` collector".to_string(),
            });
        }
        let keyword_names: Vec<_> = kwargs
            .iter()
            .filter(|argument| !argument.is_forwarded())
            .map(|arg| arg.name.as_str())
            .collect();
        let matched = match_call_slots(
            &signature.names,
            &signature.required,
            signature.positional_only,
            signature.keyword_only,
            args.len(),
            &keyword_names,
            CallVariadics {
                positional: variadic.is_some(),
                keyword: kw_variadic.is_some(),
            },
        )
        .map_err(|error| error.into_type_error(name))?;
        let mut patterns = Vec::new();
        let mut actuals = Vec::new();
        for (index, slot) in matched.slots.iter().enumerate() {
            let expression = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            patterns.push(params[index].clone());
            actuals.push(self.infer(expression)?);
        }
        if let Some(element) = variadic {
            for position in matched.positional_overflow {
                patterns.push(element.clone());
                actuals.push(self.infer(&args[position])?);
            }
        }
        if let Some(element) = kw_variadic {
            for position in matched.keyword_overflow {
                patterns.push(element.clone());
                actuals.push(self.infer(&kwargs[position].value)?);
            }
            if let Some(actual) = forwarded_element {
                patterns.push(element.clone());
                actuals.push(actual);
            }
        }
        let (subst, tyargs) =
            self.resolve_use_params(name, &signature.decls, param_args, &patterns, &actuals)?;
        let values = Self::value_argument_environment(&signature.decls, &tyargs);
        let resolve = |ty: &Ty| {
            let substituted = self.resolve_assoc_ty(&substitute(ty, &subst));
            self.resolve_dependent_ty(&substituted, &values)
        };
        let arguments = signature
            .decls
            .iter()
            .zip(tyargs.iter().cloned())
            .map(|(decl, argument)| (decl.name().trim_start_matches('*').to_string(), argument))
            .collect();
        Ok((
            params.iter().map(resolve).collect::<Result<Vec<_>, _>>()?,
            variadic.map(resolve).transpose()?,
            kw_variadic.map(resolve).transpose()?,
            subst,
            arguments,
        ))
    }

    pub(super) fn method_constraints_apply(
        &self,
        signature: &MethodSig,
        arguments: &HashMap<String, TyArg>,
    ) -> bool {
        self.method_constraint_result(signature, arguments).is_ok()
    }

    /// Evaluate method availability while preserving an outer diagnostic message.
    /// `Err(None)` is an ordinary failed constraint; `Err(Some(_))` is the current
    /// `(condition, "message")` form and may explain a sole-candidate call failure.
    pub(super) fn method_constraint_result<'signature>(
        &self,
        signature: &'signature MethodSig,
        arguments: &HashMap<String, TyArg>,
    ) -> Result<(), Option<&'signature str>> {
        let borrowed: HashMap<&str, &TyArg> = arguments
            .iter()
            .map(|(name, argument)| (name.as_str(), argument))
            .collect();
        for constraint in &signature.availability {
            if !self.eval_generic_constraint(constraint, &borrowed) {
                let message = match constraint {
                    GenericConstraint::WithMessage(_, message) => Some(message.as_str()),
                    _ => None,
                };
                return Err(message);
            }
        }
        Ok(())
    }
}

/// Whether a struct symbol names the bundled `Span` view (bare or
/// module-qualified).
fn is_span_struct_name(name: &str) -> bool {
    name == "Span" || name.ends_with("$Span")
}

/// The declaration-order compile-time arguments a generic method call
/// resolved, when the signature declares any (the name-keyed map is the
/// solver's shape; declaration order is the specialization identity).
pub(super) fn method_instantiation_arguments(
    signature: &MethodSig,
    arguments: &HashMap<String, TyArg>,
) -> Option<Vec<TyArg>> {
    if signature.decls.is_empty() {
        return None;
    }
    signature
        .decls
        .iter()
        .map(|decl| {
            arguments
                .get(decl.name().trim_start_matches('*'))
                .map(materialized_instantiation_argument)
        })
        .collect()
}

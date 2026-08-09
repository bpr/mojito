//! Generic parameter classification, solving, substitution, and bound checks.

use super::*;

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
        // A non-parameter pattern contributes no solution; coercion is checked
        // separately by the caller.
        _ => Ok(()),
    }
}

/// Replace every `Ty::Param` in `ty` with its solution from `subst` (leaving an
/// unsolved parameter untouched). Recurses into struct type arguments.
pub(super) fn substitute(ty: &Ty, subst: &HashMap<String, Ty>) -> Ty {
    match ty {
        Ty::Param {
            name,
            bounds,
            callable_bound,
        } => subst.get(name).cloned().unwrap_or_else(|| Ty::Param {
            name: name.clone(),
            bounds: bounds.clone(),
            callable_bound: callable_bound
                .as_ref()
                .map(|bound| Box::new(substitute(bound, subst))),
        }),
        Ty::Struct(name, args) => {
            Ty::Struct(name.clone(), map_tyargs(args, |t| substitute(t, subst)))
        }
        Ty::Dependent(crate::types::DependentType::Indexed { elements, index }) => {
            Ty::Dependent(crate::types::DependentType::Indexed {
                elements: elements.iter().map(|ty| substitute(ty, subst)).collect(),
                index: index.clone(),
            })
        }
        Ty::ComptimeList(elem) => Ty::ComptimeList(Box::new(substitute(elem, subst))),
        Ty::Tuple(elems) => Ty::Tuple(elems.iter().map(|t| substitute(t, subst)).collect()),
        Ty::RuntimePack(elems) => {
            Ty::RuntimePack(elems.iter().map(|t| substitute(t, subst)).collect())
        }
        Ty::VariadicPack(element) => Ty::VariadicPack(Box::new(substitute(element, subst))),
        Ty::Variant(alternatives) => Ty::Variant(
            alternatives
                .iter()
                .map(|ty| substitute(ty, subst))
                .collect(),
        ),
        Ty::Pointer { element, origin } => Ty::Pointer {
            element: Box::new(substitute(element, subst)),
            origin: origin.clone(),
        },
        Ty::Ref(reference) => {
            let mut reference = reference.clone();
            reference.referent = Box::new(substitute(&reference.referent, subst));
            Ty::Ref(reference)
        }
        Ty::Assoc { base, name, args } => Ty::Assoc {
            base: Box::new(substitute(base, subst)),
            name: name.clone(),
            args: map_tyargs(args, |t| substitute(t, subst)),
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
            params: params.iter().map(|p| substitute(p, subst)).collect(),
            names: names.clone(),
            ret: Box::new(substitute(ret, subst)),
            required: required.clone(),
            variadic: variadic.as_ref().map(|v| Box::new(substitute(v, subst))),
            kw_variadic: kw_variadic.as_ref().map(|v| Box::new(substitute(v, subst))),
            positional_only: *positional_only,
            keyword_only: *keyword_only,
            raises: *raises,
            error: error
                .as_ref()
                .map(|error| Box::new(substitute(error, subst))),
            conventions: conventions.clone(),
            ref_params: ref_params.clone(),
            ref_return: ref_return.clone(),
            transfers: transfers.clone(),
        },
        Ty::GenericFunc {
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
        } => {
            // An anonymous callable's own binders shadow names from the
            // surrounding substitution. Outer parameters may still occur in
            // its bounds and signature, so substitute with only those shadowed
            // entries removed.
            let mut nested = subst.clone();
            for declaration in decls {
                nested.remove(declaration.name());
            }
            let decls = decls
                .iter()
                .map(|declaration| match declaration {
                    ParamDecl::Type {
                        name,
                        bounds,
                        callable_bound,
                        default,
                        infer_only,
                        variadic,
                        constraints,
                    } => ParamDecl::Type {
                        name: name.clone(),
                        bounds: bounds.clone(),
                        callable_bound: callable_bound
                            .as_ref()
                            .map(|bound| Box::new(substitute(bound, &nested))),
                        default: default
                            .as_ref()
                            .map(|default| Box::new(substitute(default, &nested))),
                        infer_only: *infer_only,
                        variadic: *variadic,
                        constraints: constraints.clone(),
                    },
                    ParamDecl::Value {
                        name,
                        ty,
                        default,
                        callable_default,
                        infer_only,
                        variadic,
                        constraints,
                    } => ParamDecl::Value {
                        name: name.clone(),
                        ty: Box::new(substitute(ty, &nested)),
                        default: default.clone(),
                        callable_default: callable_default.clone(),
                        infer_only: *infer_only,
                        variadic: *variadic,
                        constraints: constraints.clone(),
                    },
                })
                .collect();
            Ty::GenericFunc {
                environment: environment.clone(),
                decls,
                params: params
                    .iter()
                    .map(|parameter| substitute(parameter, &nested))
                    .collect(),
                names: names.clone(),
                ret: Box::new(substitute(ret, &nested)),
                required: required.clone(),
                variadic: variadic
                    .as_ref()
                    .map(|parameter| Box::new(substitute(parameter, &nested))),
                kw_variadic: kw_variadic
                    .as_ref()
                    .map(|parameter| Box::new(substitute(parameter, &nested))),
                positional_only: *positional_only,
                keyword_only: *keyword_only,
                raises: *raises,
                error: error
                    .as_ref()
                    .map(|error| Box::new(substitute(error, &nested))),
                conventions: conventions.clone(),
                ref_params: ref_params.clone(),
                ref_return: ref_return.clone(),
                transfers: transfers.clone(),
            }
        }
        Ty::Overload(candidates) => Ty::Overload(
            candidates
                .iter()
                .map(|candidate| substitute(candidate, subst))
                .collect(),
        ),
        _ => ty.clone(),
    }
}

/// Alpha-rename compile-time value binders referenced by structural dependent
/// types. Type-parameter substitution and value-parameter renaming are kept
/// separate: a value binder occurs inside [`CtExpr`], never as `Ty::Param`.
/// Nested generic callable declarations shadow an outer binder of the same
/// spelling, so only genuinely free references are renamed while descending.
pub(super) fn rename_dependent_parameters(ty: &Ty, names: &HashMap<String, String>) -> Ty {
    match ty {
        Ty::Param {
            name,
            bounds,
            callable_bound,
        } => Ty::Param {
            name: name.clone(),
            bounds: bounds.clone(),
            callable_bound: callable_bound
                .as_ref()
                .map(|bound| Box::new(rename_dependent_parameters(bound, names))),
        },
        Ty::Struct(name, arguments) => Ty::Struct(
            name.clone(),
            map_tyargs(arguments, |ty| rename_dependent_parameters(ty, names)),
        ),
        Ty::Dependent(crate::types::DependentType::Indexed { elements, index }) => {
            Ty::Dependent(crate::types::DependentType::Indexed {
                elements: elements
                    .iter()
                    .map(|ty| rename_dependent_parameters(ty, names))
                    .collect(),
                index: index.rename_parameters(names),
            })
        }
        Ty::ComptimeList(element) => {
            Ty::ComptimeList(Box::new(rename_dependent_parameters(element, names)))
        }
        Ty::Tuple(elements) => Ty::Tuple(
            elements
                .iter()
                .map(|ty| rename_dependent_parameters(ty, names))
                .collect(),
        ),
        Ty::RuntimePack(elements) => Ty::RuntimePack(
            elements
                .iter()
                .map(|ty| rename_dependent_parameters(ty, names))
                .collect(),
        ),
        Ty::VariadicPack(element) => {
            Ty::VariadicPack(Box::new(rename_dependent_parameters(element, names)))
        }
        Ty::Variant(alternatives) => Ty::Variant(
            alternatives
                .iter()
                .map(|ty| rename_dependent_parameters(ty, names))
                .collect(),
        ),
        Ty::Pointer { element, origin } => Ty::Pointer {
            element: Box::new(rename_dependent_parameters(element, names)),
            origin: origin.clone(),
        },
        Ty::Ref(reference) => {
            let mut reference = reference.clone();
            reference.referent = Box::new(rename_dependent_parameters(&reference.referent, names));
            Ty::Ref(reference)
        }
        Ty::Assoc { base, name, args } => Ty::Assoc {
            base: Box::new(rename_dependent_parameters(base, names)),
            name: name.clone(),
            args: map_tyargs(args, |t| rename_dependent_parameters(t, names)),
        },
        Ty::Func {
            environment,
            params,
            names: parameter_names,
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
                .map(|ty| rename_dependent_parameters(ty, names))
                .collect(),
            names: parameter_names.clone(),
            ret: Box::new(rename_dependent_parameters(ret, names)),
            required: required.clone(),
            variadic: variadic
                .as_ref()
                .map(|ty| Box::new(rename_dependent_parameters(ty, names))),
            kw_variadic: kw_variadic
                .as_ref()
                .map(|ty| Box::new(rename_dependent_parameters(ty, names))),
            positional_only: *positional_only,
            keyword_only: *keyword_only,
            raises: *raises,
            error: error
                .as_ref()
                .map(|ty| Box::new(rename_dependent_parameters(ty, names))),
            conventions: conventions.clone(),
            ref_params: ref_params.clone(),
            ref_return: ref_return.clone(),
            transfers: transfers.clone(),
        },
        Ty::GenericFunc {
            environment,
            decls,
            params,
            names: parameter_names,
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
        } => {
            let mut free_names = names.clone();
            for declaration in decls {
                free_names.remove(declaration.name().trim_start_matches('*'));
            }
            let decls = decls
                .iter()
                .map(|declaration| match declaration {
                    ParamDecl::Type {
                        name,
                        bounds,
                        callable_bound,
                        default,
                        infer_only,
                        variadic,
                        constraints,
                    } => ParamDecl::Type {
                        name: name.clone(),
                        bounds: bounds.clone(),
                        callable_bound: callable_bound
                            .as_ref()
                            .map(|bound| Box::new(rename_dependent_parameters(bound, &free_names))),
                        default: default.as_ref().map(|default| {
                            Box::new(rename_dependent_parameters(default, &free_names))
                        }),
                        infer_only: *infer_only,
                        variadic: *variadic,
                        constraints: constraints.clone(),
                    },
                    ParamDecl::Value {
                        name,
                        ty,
                        default,
                        callable_default,
                        infer_only,
                        variadic,
                        constraints,
                    } => ParamDecl::Value {
                        name: name.clone(),
                        ty: Box::new(rename_dependent_parameters(ty, &free_names)),
                        default: default
                            .as_ref()
                            .map(|value| value.rename_parameters(&free_names)),
                        callable_default: callable_default.clone(),
                        infer_only: *infer_only,
                        variadic: *variadic,
                        constraints: constraints.clone(),
                    },
                })
                .collect();
            Ty::GenericFunc {
                environment: environment.clone(),
                decls,
                params: params
                    .iter()
                    .map(|ty| rename_dependent_parameters(ty, &free_names))
                    .collect(),
                names: parameter_names.clone(),
                ret: Box::new(rename_dependent_parameters(ret, &free_names)),
                required: required.clone(),
                variadic: variadic
                    .as_ref()
                    .map(|ty| Box::new(rename_dependent_parameters(ty, &free_names))),
                kw_variadic: kw_variadic
                    .as_ref()
                    .map(|ty| Box::new(rename_dependent_parameters(ty, &free_names))),
                positional_only: *positional_only,
                keyword_only: *keyword_only,
                raises: *raises,
                error: error
                    .as_ref()
                    .map(|ty| Box::new(rename_dependent_parameters(ty, &free_names))),
                conventions: conventions.clone(),
                ref_params: ref_params.clone(),
                ref_return: ref_return.clone(),
                transfers: transfers.clone(),
            }
        }
        Ty::Overload(candidates) => Ty::Overload(
            candidates
                .iter()
                .map(|ty| rename_dependent_parameters(ty, names))
                .collect(),
        ),
        _ => ty.clone(),
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
        Ty::Dependent(crate::types::DependentType::Indexed { elements, index }) => {
            Ty::Dependent(crate::types::DependentType::Indexed {
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

/// Apply `f` to each type argument of a struct's parameter list, passing value
/// arguments through unchanged.
pub(super) fn map_tyargs(args: &[TyArg], mut f: impl FnMut(&Ty) -> Ty) -> Vec<TyArg> {
    args.iter()
        .map(|a| match a {
            TyArg::Ty(t) => TyArg::Ty(f(t)),
            TyArg::Val(v) => TyArg::Val(v.clone()),
            // Origin substitution is threaded separately; pass origins through.
            TyArg::Origin(o) => TyArg::Origin(o.clone()),
        })
        .collect()
}

/// The bindings concrete resolution of a parameterized associated type
/// substitutes into its symbolic template: type parameters by name (the
/// enclosing struct's and the member's), value parameters by name, and origin
/// parameters by `OriginParamId`.
pub(super) struct AssocBindings {
    pub types: HashMap<String, Ty>,
    pub values: HashMap<String, CtValue>,
    pub origins: HashMap<u32, crate::origin::Origin>,
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
    origins: &HashMap<u32, crate::origin::Origin>,
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
        Ty::Dependent(crate::types::DependentType::Indexed { elements, index }) => {
            Ty::Dependent(crate::types::DependentType::Indexed {
                elements: elements.iter().map(recur).collect(),
                index: index.clone(),
            })
        }
        other => other.clone(),
    }
}

fn substitute_origin(
    origin: &crate::origin::Origin,
    origins: &HashMap<u32, crate::origin::Origin>,
) -> crate::origin::Origin {
    use crate::origin::Origin;
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
        arguments: &[crate::ast::ParamArg],
        signature: &CallableOriginSignature,
    ) -> Result<SplitCallableSpecialization, TypeError> {
        use crate::ast::ParamArg;

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
                        crate::checked::SemanticAdjustment::EraseCompileTimeArgument,
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
        bindings: &[(Vec<usize>, crate::origin::Origin)],
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
        arguments: &[crate::ast::ParamArg],
        callable: Ty,
        signature: Option<&CallableOriginSignature>,
    ) -> Result<(Ty, Vec<crate::ast::ParamArg>), TypeError> {
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
        arguments: &[crate::ast::ParamArg],
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
        arguments: &[crate::ast::ParamArg],
        callable: Ty,
        signature: Option<&CallableOriginSignature>,
    ) -> Result<Ty, TypeError> {
        let (callable, ordinary) =
            self.prepare_callable_specialization(name, arguments, callable, signature)?;
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
        arguments: &[crate::ast::ParamArg],
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

    #[allow(clippy::too_many_arguments)]
    pub(super) fn instantiate_method_generics(
        &self,
        name: &str,
        signature: &MethodSig,
        params: &[Ty],
        variadic: Option<&Ty>,
        kw_variadic: Option<&Ty>,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
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
        let borrowed: HashMap<&str, &TyArg> = arguments
            .iter()
            .map(|(name, argument)| (name.as_str(), argument))
            .collect();
        signature
            .availability
            .iter()
            .all(|constraint| self.eval_generic_constraint(constraint, &borrowed))
    }
}

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
        Ty::Assoc { base, name } => Ty::Assoc {
            base: Box::new(substitute(base, subst)),
            name: name.clone(),
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
        Ty::Assoc { base, name } => Ty::Assoc {
            base: Box::new(rename_dependent_parameters(base, names)),
            name: name.clone(),
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
        Ty::Assoc { base, name } => Ty::Assoc {
            base: Box::new(substitute_self(base, replacement)),
            name: name.clone(),
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
        })
        .collect()
}

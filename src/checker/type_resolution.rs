//! Resolution of source type annotations into checked `Ty` values:
//! type-argument-bearing builtins (List/Set/Dict/Pointer/Tuple/SIMD/Variant),
//! dependent/associated type projection, and type-parameter lookup.
//! Extracted from `checker.rs`; see `docs/symbol-map.md`.

use super::*;

impl Checker {
    /// The type denoted by a source annotation; resolves type parameters and
    /// validates struct names and type-argument counts.
    pub(super) fn ty_from_anno(&self, ty: &SourceType) -> Result<Ty, TypeError> {
        self.resolve_ty_from_anno(ty)
    }

    pub(super) fn resolve_ty_from_anno(&self, ty: &SourceType) -> Result<Ty, TypeError> {
        Ok(match ty {
            SourceType::Int => Ty::Int,
            SourceType::UInt => Ty::UInt,
            SourceType::Bool => Ty::Bool,
            SourceType::String => Ty::String,
            SourceType::Float64 => Ty::Float64,
            SourceType::None => Ty::None,
            SourceType::Func {
                type_params,
                params,
                ret,
                thin,
                capturing,
                raises,
                raises_type,
            } => {
                let environment =
                    self.lower_callable_environment(type_params, *thin, capturing.as_ref())?;
                let function_params: Vec<FnParam> = params
                    .iter()
                    .enumerate()
                    .map(|(index, parameter)| FnParam {
                        name: parameter
                            .name
                            .clone()
                            .unwrap_or_else(|| format!("arg{index}")),
                        ty: parameter.ty.clone(),
                        default: None,
                        kind: crate::ast::ParamKind::Regular,
                        convention: parameter.convention,
                        origin: parameter.origin.clone(),
                    })
                    .collect();
                let regular: Vec<&FnParam> = function_params.iter().collect();
                let parameter_types = function_params
                    .iter()
                    .map(|parameter| self.resolve_ty_from_anno(&parameter.ty))
                    .collect::<Result<Vec<_>, _>>()?;
                let (return_type, ref_return) = match &**ret {
                    SourceType::Ref { referent, origin } => (
                        self.resolve_ty_from_anno(referent)?,
                        Some(Box::new(self.lower_callable_ref_sig(
                            origin.as_ref().ok_or_else(|| {
                                TypeError::Unsupported(
                                    "reference return requires an origin".to_string(),
                                )
                            })?,
                            type_params,
                            &regular,
                        )?)),
                    ),
                    return_type => (self.resolve_ty_from_anno(return_type)?, None),
                };
                Ty::Func {
                    environment,
                    params: parameter_types,
                    names: function_params
                        .iter()
                        .map(|parameter| parameter.name.clone())
                        .collect(),
                    ret: Box::new(return_type),
                    required: vec![true; params.len()],
                    variadic: None,
                    kw_variadic: None,
                    positional_only: None,
                    keyword_only: None,
                    raises: *raises,
                    error: if *raises {
                        Some(Box::new(match raises_type {
                            Some(error) => self.resolve_ty_from_anno(error)?,
                            None => Ty::Error,
                        }))
                    } else {
                        None
                    },
                    conventions: function_params
                        .iter()
                        .map(|parameter| parameter.convention)
                        .collect(),
                    ref_params: Box::new(
                        self.lower_callable_ref_param_sigs(type_params, &regular)?,
                    ),
                    ref_return,
                }
            }
            SourceType::MaterializedCallable(key) => {
                let callable = self.materialized_callables.get(key).ok_or_else(|| {
                    TypeError::InvariantViolation(format!(
                        "compiler-generated callable annotation has unknown id '{key}'"
                    ))
                })?;
                if !matches!(callable, Ty::Func { .. } | Ty::GenericFunc { .. }) {
                    return Err(TypeError::InvariantViolation(
                        "compiler-generated callable annotation contains a non-callable type"
                            .to_string(),
                    ));
                }
                callable.clone()
            }
            SourceType::Ref { referent, origin } => {
                let spec = origin.as_ref().ok_or_else(|| {
                    TypeError::Unsupported(
                        "reference-valued fields require an explicit origin".to_string(),
                    )
                })?;
                let [origin_expr] = spec.as_slice() else {
                    return Err(TypeError::Unsupported(
                        "reference-valued fields currently require one origin parameter"
                            .to_string(),
                    ));
                };
                let ExprKind::Identifier(origin_name) = &origin_expr.kind else {
                    return Err(TypeError::Unsupported(
                        "reference-valued fields require a named origin parameter".to_string(),
                    ));
                };
                if origin_name == "UnsafeAnyOrigin" {
                    return Err(TypeError::Unsupported(
                        "UnsafeAnyOrigin cannot be hidden in a stored reference field".to_string(),
                    ));
                }
                if origin_name == "UntrackedOrigin" {
                    return Ok(Ty::Ref(crate::origin::RefTy {
                        referent: Box::new(self.resolve_ty_from_anno(referent)?),
                        origin: crate::origin::Origin::Untracked { mutable: false },
                        mutability: crate::origin::Mutability::Immutable,
                    }));
                }
                let (index, parameter) = self
                    .enclosing_type_params
                    .iter()
                    .enumerate()
                    .find(|(_, parameter)| {
                        parameter.name == *origin_name && parameter.bounds.as_slice() == ["Origin"]
                    })
                    .ok_or_else(|| TypeError::UndefinedVariable(origin_name.clone()))?;
                let mutability = match parameter.origin_mutability.as_ref().map(|e| &e.kind) {
                    Some(ExprKind::Bool(true)) => crate::origin::Mutability::Mutable,
                    Some(ExprKind::Bool(false)) => crate::origin::Mutability::Immutable,
                    _ => {
                        crate::origin::Mutability::Param(crate::origin::OriginParamId(index as u32))
                    }
                };
                Ty::Ref(crate::origin::RefTy {
                    referent: Box::new(self.resolve_ty_from_anno(referent)?),
                    origin: crate::origin::Origin::Param(crate::origin::OriginParamId(
                        index as u32,
                    )),
                    mutability,
                })
            }
            // A bare name may be an in-scope type parameter (a generic `def`'s
            // `T`) or a struct type, optionally applied to parameter arguments.
            SourceType::Named(name, args) => {
                let existential_trait = args.first().and_then(|argument| match argument {
                    crate::ast::ParamArg::Type(SourceType::Named(trait_name, trait_args))
                        if trait_args.is_empty() =>
                    {
                        Some(trait_name)
                    }
                    crate::ast::ParamArg::Value(Expr {
                        kind: ExprKind::Identifier(trait_name),
                        ..
                    }) => Some(trait_name),
                    _ => None,
                });
                if name == "Some"
                    && args.len() == 1
                    && let Some(trait_name) = existential_trait
                    && (BUILTIN_TRAITS.contains(&trait_name.as_str())
                        || self.traits.contains_key(trait_name))
                {
                    return Ok(Ty::Param {
                        name: format!("Some[{trait_name}]"),
                        bounds: vec![trait_name.clone()],
                        callable_bound: None,
                    });
                }
                if name == "Never" && args.is_empty() {
                    return Ok(Ty::Never);
                }
                if name == "NoneType" && args.is_empty() {
                    return Ok(Ty::None);
                }
                if matches!(name.as_str(), "Slice" | "ContiguousSlice" | "StridedSlice")
                    && args.is_empty()
                {
                    return Ok(Ty::Struct(name.clone(), Vec::new()));
                }
                // Mojo exposes the compile-time `StringLiteral` type. Mojito
                // materializes string literals directly as runtime strings, so
                // it is represented by the existing string type.
                if name == "StringLiteral" && args.is_empty() {
                    return Ok(Ty::String);
                }
                if args.is_empty()
                    && let Some(parameter) = self.lookup_tparam(name)
                {
                    return Ok(parameter);
                }
                // SIMD vector types and their fixed-width scalar aliases.
                if let Some(dtype) = Dtype::from_scalar_alias(name) {
                    if !args.is_empty() {
                        return Err(TypeError::WrongTypeArgCount {
                            name: name.clone(),
                            expected: 0,
                            got: args.len(),
                        });
                    }
                    return Ok(Ty::Simd { dtype, width: 1 });
                }
                if name == "SIMD" {
                    return self.simd_type(args);
                }
                if name == "Scalar" {
                    if args.len() != 1 {
                        return Err(TypeError::WrongTypeArgCount {
                            name: name.clone(),
                            expected: 1,
                            got: args.len(),
                        });
                    }
                    return Ok(simd_ty(dtype_from_arg(&args[0])?, 1));
                }
                if name == "$pack" {
                    return self.tuple_element_types(args).map(Ty::RuntimePack);
                }
                if name == "__RuntimeTuple" {
                    return self.tuple_element_types(args).map(Ty::Tuple);
                }
                if name == "_" && args.is_empty() {
                    return Ok(Ty::Infer);
                }
                if name == "Error" && args.is_empty() {
                    return Ok(Ty::Error);
                }
                // `Variant` is a compiler-provided tagged union even when its
                // stdlib declaration has been module-qualified by the linker.
                if is_variant_name(name) && (name != "Variant" || self.structs.contains_key(name)) {
                    return self.variant_type(args);
                }
                // Literal families are lang items: direct `_` holes are solved
                // from an initializer before ordinary generic-bound checking.
                if name == "List" {
                    return self.list_type(args);
                }
                if name == "Set" {
                    return self.set_type(args);
                }
                if name == "Dict" {
                    return self.dict_type(args);
                }
                if name == "Tuple" {
                    return self.tuple_type(args);
                }
                // A generated public-Tuple name keeps its forward-type
                // resolution even once its shell is registered: its source
                // parameter list is erased, so ordinary declaration binding
                // cannot accept the semantic arguments the mangled reference
                // spells.
                if self.allow_generated_tuple_forward_types
                    && (name.starts_with("Tuple$") || name.contains("$Tuple$"))
                    && self.declared_structs.contains(name)
                {
                    return self.generated_tuple_forward_type(name, args);
                }
                if let Some(info) = self.structs.get(name) {
                    let decls = info.decls.clone();
                    let (_, tyargs) = self.resolve_use_params(name, &decls, args, &[], &[])?;
                    return Ok(self.struct_instance_type(name, tyargs));
                }
                if self.allow_generated_tuple_forward_types && self.declared_structs.contains(name)
                {
                    return self.generated_tuple_forward_type(name, args);
                }
                if matches!(name.as_str(), "UnsafePointer" | "Pointer") {
                    return self.pointer_type(args);
                }
                return Err(TypeError::UnknownType(name.clone()));
            }
            // `Self.T` — one of the enclosing struct's *type* parameters (a value
            // parameter is not a type, so `Self.n` in type position is an error).
            SourceType::SelfParam(name) => {
                match self.self_decls.iter().find(|d| d.name() == name) {
                    Some(ParamDecl::Type {
                        bounds,
                        callable_bound,
                        ..
                    }) => Ty::Param {
                        name: name.clone(),
                        bounds: bounds.clone(),
                        callable_bound: callable_bound.clone(),
                    },
                    _ => return self.associated_type_for_self(name),
                }
            }
            // Bare `Self` — the enclosing struct type or a trait's abstract Self.
            // Not usable as a type in a value-parameterized struct (a value
            // parameter can't appear in a type).
            SourceType::SelfType => match &self.self_ty {
                Some(Ty::Struct(_, args)) if args.iter().any(|a| matches!(a, TyArg::Val(_))) => {
                    return Err(TypeError::UnknownSelfParam("Self".to_string()));
                }
                Some(ty) => ty.clone(),
                None => return Err(TypeError::UnknownSelfParam("Self".to_string())),
            },
            SourceType::Assoc { base, name, .. } => {
                let base_ty = self.ty_from_anno(base)?;
                self.associated_type_from_base(&base_ty, name, &[])?
            }
            SourceType::IndexedProjection { base, index } => {
                // A parameterized associated-type application such as
                // `Self.IteratorType[origin_of(self)]` is spelled like a dependent
                // index but names a parameterized associated member; resolve it as
                // an application rather than compile-time sequence indexing.
                let application = match base.as_ref() {
                    // `Self.IteratorType[..]` — the abstract trait `Self` in a trait
                    // method, or the concrete struct when a conformer spells the
                    // application as its own return type. Use the concrete `Self`
                    // (a registered struct) when available so the member resolves
                    // concretely; otherwise the trait's abstract `Self`.
                    SourceType::SelfParam(name) => {
                        let base = match &self.self_ty {
                            Some(ty @ Ty::Struct(..)) => ty.clone(),
                            _ => Ty::SelfType,
                        };
                        self.parameterized_assoc_application(
                            &base,
                            name,
                            std::slice::from_ref(index),
                        )?
                    }
                    // `C.IteratorType[..]` — a bounded type parameter or other base.
                    // If the base does not name a type (e.g. `values.element_types`
                    // where `values` is a value binding), this is not an
                    // associated-type application; fall through to dependent
                    // sequence indexing instead of surfacing a "not a type" error.
                    SourceType::Assoc {
                        base: inner, name, ..
                    } => match self.ty_from_anno(inner) {
                        Ok(base_ty) => self.parameterized_assoc_application(
                            &base_ty,
                            name,
                            std::slice::from_ref(index),
                        )?,
                        Err(_) => None,
                    },
                    _ => None,
                };
                if let Some(applied) = application {
                    applied
                } else {
                    let elements = self.dependent_type_sequence(base)?;
                    let index = self.compile_dependent_ct_expr(index)?;
                    self.resolve_dependent_index(elements, index, &HashMap::new())?
                }
            }
        })
    }

    /// The parameter list of a parameterized associated type `name` reachable from
    /// a base type — a trait's abstract `Self` or a bounded type parameter — or
    /// `None` when the member is monomorphic, absent, or on a concrete struct
    /// (whose concrete resolution is handled separately).
    fn parameterized_assoc_params(
        &self,
        base_ty: &Ty,
        name: &str,
    ) -> Option<Vec<crate::ast::TypeParam>> {
        match base_ty {
            Ty::SelfType => self
                .trait_self_comptime
                .last()
                .and_then(|reqs| reqs.get(name))
                .and_then(|req| match req {
                    CtMemberReq::Type { params, .. } if !params.is_empty() => Some(params.clone()),
                    _ => None,
                }),
            Ty::Param { bounds, .. } => self.lookup_trait_assoc_params(bounds, name),
            // A concrete struct that instantiates the member: its parameter list
            // comes from the struct's own parameterized definition, so an
            // application such as a conformer's `Self.IteratorType[origin_of(self)]`
            // resolves concretely rather than falling through to dependent indexing.
            Ty::Struct(sname, _) => self
                .structs
                .get(sname)
                .and_then(|info| info.parameterized_associated.get(name))
                .map(|member| member.params.clone()),
            _ => None,
        }
    }

    /// Resolve a parameterized associated-type application `base.name[args]`.
    /// Returns `None` when `name` is not a parameterized associated member of
    /// `base` (so the caller falls back to dependent sequence indexing). The
    /// result is a symbolic associated type; concrete resolution happens once the
    /// base is a conforming struct (a later iteration subtask).
    fn parameterized_assoc_application(
        &self,
        base_ty: &Ty,
        name: &str,
        args: &[Expr],
    ) -> Result<Option<Ty>, TypeError> {
        let Some(params) = self.parameterized_assoc_params(base_ty, name) else {
            return Ok(None);
        };
        // Explicit parameters (after the `//` infer-only marker) are supplied at
        // the application; infer-only parameters are derived from them.
        let explicit: Vec<&crate::ast::TypeParam> =
            params.iter().filter(|p| !p.infer_only).collect();
        if args.len() != explicit.len() {
            return Err(TypeError::WrongTypeArgCount {
                name: name.to_string(),
                expected: explicit.len(),
                got: args.len(),
            });
        }
        // Carry the application arguments in the checked type. The base here is
        // still abstract (`Self` or a bounded parameter). An `origin_of(self)`
        // argument in a trait method's abstract signature has no bound `self`
        // place; it lowers to the symbolic `Origin::SelfParam`, resolved to the
        // concrete receiver origin once the base is a conforming struct. Concrete
        // substitution and per-kind argument validation happen there.
        let arguments = explicit
            .iter()
            .zip(args)
            .map(|(param, arg)| self.lower_assoc_application_arg(param, arg))
            .collect::<Result<Vec<_>, _>>()?;
        // A concrete struct base instantiates the member here and now (a conformer
        // spelling `Self.IteratorType[origin_of(self)]` as its own return type):
        // resolve it through the same path as a non-indexed struct-based
        // application. An abstract base (`Self` or a bounded parameter) stays
        // symbolic until the base becomes a conforming struct.
        if matches!(base_ty, Ty::Struct(..)) {
            return Ok(Some(
                self.associated_type_from_base(base_ty, name, &arguments)?,
            ));
        }
        Ok(Some(Ty::Assoc {
            base: Box::new(base_ty.clone()),
            name: name.to_string(),
            args: arguments,
        }))
    }

    /// Lower one application argument of a parameterized associated type into a
    /// checked `TyArg`, dispatched by the declared parameter's kind: an origin
    /// parameter takes an `origin_of(...)`/builtin origin, a value parameter a
    /// compile-time value, and a type parameter a type.
    fn lower_assoc_application_arg(
        &self,
        param: &crate::ast::TypeParam,
        arg: &Expr,
    ) -> Result<TyArg, TypeError> {
        use crate::ast::ParamArg;
        if matches!(param.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet") {
            return Ok(TyArg::Origin(
                self.explicit_origin_argument(&ParamArg::Value(arg.clone()))?,
            ));
        }
        let is_value = param.value_type.is_some()
            || matches!(param.bounds.as_slice(), [only] if scalar_type_name(only).is_some());
        if is_value {
            return Ok(TyArg::Val(self.eval_associated_ct(arg, &HashMap::new())?));
        }
        // A type parameter: a bare identifier names a type; otherwise the
        // argument must evaluate to a compile-time type value.
        let ty = match &arg.kind {
            ExprKind::Identifier(id) => {
                self.ty_from_anno(&SourceType::Named(id.clone(), Vec::new()))?
            }
            _ => match self.eval_associated_ct(arg, &HashMap::new())? {
                CtValue::Type(ty) => *ty,
                _ => {
                    return Err(TypeError::TypeMismatch {
                        expected: "a type".to_string(),
                        found: "a value".to_string(),
                        context: format!("associated type parameter '{}'", param.name),
                    });
                }
            },
        };
        Ok(TyArg::Ty(ty))
    }

    /// Resolve only the nominal identity embedded in a compiler-generated
    /// Tuple's concrete metadata. Full parameter arity/bound validation still
    /// occurs at the user's original type use during discovery; this path exists
    /// solely because the generated implementation may be emitted before that
    /// already-checked user struct declaration.
    pub(super) fn generated_tuple_forward_type(
        &self,
        name: &str,
        arguments: &[crate::ast::ParamArg],
    ) -> Result<Ty, TypeError> {
        fn argument(checker: &Checker, value: &crate::ast::ParamArg) -> Result<TyArg, TypeError> {
            match value {
                crate::ast::ParamArg::Type(ty) => checker.ty_from_anno(ty).map(TyArg::Ty),
                crate::ast::ParamArg::Value(value) => checker
                    .eval_associated_ct(value, &HashMap::new())
                    .map(TyArg::Val),
                crate::ast::ParamArg::Named { value, .. } => argument(checker, value),
            }
        }

        let arguments = if arguments.is_empty() {
            self.predeclared_generated_tuple_arguments
                .get(name)
                .cloned()
                .unwrap_or_default()
        } else {
            arguments
                .iter()
                .map(|value| argument(self, value))
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(Ty::Struct(name.to_string(), arguments))
    }

    /// Resolve the type-valued sequence at the base of an indexed type
    /// projection. A source value may expose such a sequence through an
    /// associated compile-time member; its runtime value is never inspected.
    pub(super) fn dependent_type_sequence(
        &self,
        projection: &SourceType,
    ) -> Result<Vec<Ty>, TypeError> {
        let SourceType::Assoc { base, name, .. } = projection else {
            return Err(TypeError::Unsupported(
                "dependent type indexing requires a type-valued associated member".to_string(),
            ));
        };
        let base_ty = match base.as_ref() {
            SourceType::Named(binding, arguments) if arguments.is_empty() => self
                .lookup(binding)
                .cloned()
                .map(Ok)
                .unwrap_or_else(|| self.ty_from_anno(base))?,
            _ => self.ty_from_anno(base)?,
        };

        // Public Tuple exposes its concrete element pack as `element_types`.
        // Use the checked nominal arguments, never its generated symbol text.
        if name == "element_types"
            && let Some(elements) = tuple_elements(&base_ty)
        {
            return Ok(elements.into_iter().cloned().collect());
        }

        let Ty::Struct(struct_name, arguments) = &base_ty else {
            return Err(TypeError::NoSuchAssociatedType {
                object_type: base_ty.to_string(),
                member: name.clone(),
            });
        };
        let info = self
            .structs
            .get(struct_name)
            .ok_or_else(|| TypeError::UnknownType(struct_name.clone()))?;
        let value = info
            .associated
            .get(name)
            .ok_or_else(|| TypeError::NoSuchAssociatedType {
                object_type: base_ty.to_string(),
                member: name.clone(),
            })?;
        let values = match value {
            CtValue::Tuple(values) | CtValue::List(values) => values,
            _ => {
                return Err(TypeError::NoSuchAssociatedType {
                    object_type: base_ty.to_string(),
                    member: name.clone(),
                });
            }
        };
        let substitution = struct_subst(&info.decls, arguments);
        values
            .iter()
            .map(|value| match value {
                CtValue::Type(ty) => Ok(self.resolve_assoc_ty(&substitute(ty, &substitution))),
                _ => Err(TypeError::NotComptime(format!(
                    "{}.{} contains a non-type value",
                    base_ty, name
                ))),
            })
            .collect()
    }

    /// Collapse an indexed dependent type when its compile-time environment is
    /// concrete; otherwise retain the structural expression in generic
    /// metadata for later specialization.
    pub(super) fn resolve_dependent_index(
        &self,
        elements: Vec<Ty>,
        index: CtExpr,
        parameters: &HashMap<String, CtValue>,
    ) -> Result<Ty, TypeError> {
        let Some(value) = index.evaluate(parameters) else {
            return Ok(Ty::Dependent(DependentType::Indexed { elements, index }));
        };
        let index_value = match value {
            CtValue::Int(value) => Some(value),
            CtValue::UInt(value) => i64::try_from(value).ok(),
            CtValue::IntLiteral(value) => value.to_i64(),
            _ => None,
        }
        .ok_or_else(|| TypeError::NotComptime("dependent type index must be an Int".to_string()))?;
        let position = usize::try_from(index_value).map_err(|_| {
            TypeError::NotComptime(format!("dependent type index {index_value} is negative"))
        })?;
        elements.get(position).cloned().ok_or_else(|| {
            TypeError::NotComptime(format!(
                "dependent type index {index_value} is out of range for {} element(s)",
                elements.len()
            ))
        })
    }

    /// Resolve dependent leaves after a generic use has supplied its value
    /// parameters. This is a typed walk: the candidate type sequence and the
    /// retained [`CtExpr`] remain structural until the environment is concrete.
    pub(super) fn resolve_dependent_ty(
        &self,
        ty: &Ty,
        parameters: &HashMap<String, CtValue>,
    ) -> Result<Ty, TypeError> {
        Ok(match ty {
            Ty::Dependent(DependentType::Indexed { elements, index }) => {
                let elements = elements
                    .iter()
                    .map(|element| self.resolve_dependent_ty(element, parameters))
                    .collect::<Result<Vec<_>, _>>()?;
                self.resolve_dependent_index(elements, index.clone(), parameters)?
            }
            Ty::Struct(name, arguments) => Ty::Struct(
                name.clone(),
                arguments
                    .iter()
                    .map(|argument| match argument {
                        TyArg::Ty(ty) => self.resolve_dependent_ty(ty, parameters).map(TyArg::Ty),
                        TyArg::Val(value) => Ok(TyArg::Val(value.clone())),
                        TyArg::Origin(origin) => Ok(TyArg::Origin(origin.clone())),
                    })
                    .collect::<Result<Vec<_>, TypeError>>()?,
            ),
            Ty::ComptimeList(element) => {
                Ty::ComptimeList(Box::new(self.resolve_dependent_ty(element, parameters)?))
            }
            Ty::Tuple(elements) => Ty::Tuple(
                elements
                    .iter()
                    .map(|element| self.resolve_dependent_ty(element, parameters))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Ty::RuntimePack(elements) => Ty::RuntimePack(
                elements
                    .iter()
                    .map(|element| self.resolve_dependent_ty(element, parameters))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Ty::VariadicPack(element) => {
                Ty::VariadicPack(Box::new(self.resolve_dependent_ty(element, parameters)?))
            }
            Ty::Variant(alternatives) => Ty::Variant(
                alternatives
                    .iter()
                    .map(|alternative| self.resolve_dependent_ty(alternative, parameters))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Ty::Pointer { element, origin } => Ty::Pointer {
                element: Box::new(self.resolve_dependent_ty(element, parameters)?),
                origin: origin.clone(),
            },
            Ty::Ref(reference) => Ty::Ref(crate::origin::RefTy {
                referent: Box::new(self.resolve_dependent_ty(&reference.referent, parameters)?),
                origin: reference.origin.clone(),
                mutability: reference.mutability,
            }),
            // A nested generic callable owns its own value-binder scope. Leave
            // that scope structural here; its own invocation resolves it.
            Ty::GenericFunc { .. } => ty.clone(),
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
                    .map(|parameter| self.resolve_dependent_ty(parameter, parameters))
                    .collect::<Result<Vec<_>, _>>()?,
                names: names.clone(),
                ret: Box::new(self.resolve_dependent_ty(ret, parameters)?),
                required: required.clone(),
                variadic: variadic
                    .as_ref()
                    .map(|parameter| {
                        self.resolve_dependent_ty(parameter, parameters)
                            .map(Box::new)
                    })
                    .transpose()?,
                kw_variadic: kw_variadic
                    .as_ref()
                    .map(|parameter| {
                        self.resolve_dependent_ty(parameter, parameters)
                            .map(Box::new)
                    })
                    .transpose()?,
                positional_only: *positional_only,
                keyword_only: *keyword_only,
                raises: *raises,
                error: error
                    .as_ref()
                    .map(|error| self.resolve_dependent_ty(error, parameters).map(Box::new))
                    .transpose()?,
                conventions: conventions.clone(),
                ref_params: ref_params.clone(),
                ref_return: ref_return.clone(),
            },
            Ty::Overload(candidates) => Ty::Overload(
                candidates
                    .iter()
                    .map(|candidate| self.resolve_dependent_ty(candidate, parameters))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            _ => ty.clone(),
        })
    }

    pub(super) fn value_argument_environment(
        decls: &[ParamDecl],
        arguments: &[TyArg],
    ) -> HashMap<String, CtValue> {
        decls
            .iter()
            .zip(arguments)
            .filter_map(|(declaration, argument)| match argument {
                TyArg::Val(value) => Some((
                    declaration.name().trim_start_matches('*').to_string(),
                    value.clone(),
                )),
                TyArg::Ty(_) | TyArg::Origin(_) => None,
            })
            .collect()
    }

    pub(super) fn associated_type_for_self(&self, name: &str) -> Result<Ty, TypeError> {
        if let Some(reqs) = self.trait_self_comptime.last()
            && let Some(req) = reqs.get(name)
        {
            return match req {
                CtMemberReq::Type { .. } => Ok(Ty::Assoc {
                    base: Box::new(Ty::SelfType),
                    name: name.to_string(),
                    args: Vec::new(),
                }),
                CtMemberReq::Value(_) => Err(TypeError::NoSuchAssociatedType {
                    object_type: "Self".to_string(),
                    member: name.to_string(),
                }),
            };
        }
        let Some(self_ty) = &self.self_ty else {
            return Err(TypeError::UnknownSelfParam(name.to_string()));
        };
        if let Ty::Struct(sname, _) = self_ty
            && !self.structs.contains_key(sname)
        {
            return Err(TypeError::UnknownSelfParam(name.to_string()));
        }
        self.associated_type_from_base(self_ty, name, &[])
    }

    pub(super) fn associated_type_from_base(
        &self,
        base: &Ty,
        name: &str,
        args: &[TyArg],
    ) -> Result<Ty, TypeError> {
        match base {
            Ty::Struct(sname, targs) => {
                let info = self
                    .structs
                    .get(sname)
                    .ok_or_else(|| TypeError::UnknownType(sname.clone()))?;
                // A parameterized associated member instantiated by a conforming
                // struct: substitute the application's arguments into its template.
                if let Some(member) = info.parameterized_associated.get(name) {
                    let member = member.clone();
                    let decls = info.decls.clone();
                    let targs = targs.clone();
                    return self
                        .resolve_parameterized_member(base, name, &member, &decls, &targs, args);
                }
                let value =
                    info.associated
                        .get(name)
                        .ok_or_else(|| TypeError::NoSuchAssociatedType {
                            object_type: base.to_string(),
                            member: name.to_string(),
                        })?;
                let CtValue::Type(ty) = value else {
                    return Err(TypeError::NoSuchAssociatedType {
                        object_type: base.to_string(),
                        member: name.to_string(),
                    });
                };
                let subst = struct_subst(&info.decls, targs);
                Ok(self.resolve_assoc_ty(&substitute(ty, &subst)))
            }
            Ty::Param { bounds, .. } => {
                if self.lookup_trait_assoc_type(bounds, name).is_some() {
                    Ok(Ty::Assoc {
                        base: Box::new(base.clone()),
                        name: name.to_string(),
                        args: args.to_vec(),
                    })
                } else {
                    Err(TypeError::NoSuchAssociatedType {
                        object_type: base.to_string(),
                        member: name.to_string(),
                    })
                }
            }
            Ty::Assoc { .. } => Ok(Ty::Assoc {
                base: Box::new(base.clone()),
                name: name.to_string(),
                args: args.to_vec(),
            }),
            _ => Err(TypeError::NoSuchAssociatedType {
                object_type: base.to_string(),
                member: name.to_string(),
            }),
        }
    }

    /// Concretely resolve a parameterized associated-type application on a
    /// conforming struct. Binds the struct's own parameters (from its type
    /// arguments) and the member's parameters (from the application arguments)
    /// into the member's symbolic template. When the arguments were not carried —
    /// an `origin_of(self)` dropped at the abstract trait signature — the result
    /// stays symbolic rather than resolving with missing bindings.
    fn resolve_parameterized_member(
        &self,
        base: &Ty,
        name: &str,
        member: &ParameterizedMember,
        struct_decls: &[ParamDecl],
        struct_targs: &[TyArg],
        args: &[TyArg],
    ) -> Result<Ty, TypeError> {
        let explicit = member.params.iter().filter(|p| !p.infer_only).count();
        if args.len() != explicit {
            return Ok(Ty::Assoc {
                base: Box::new(base.clone()),
                name: name.to_string(),
                args: args.to_vec(),
            });
        }
        // The struct's own type and value parameters concretize `Self.T` / `Self.n`
        // references in the template.
        let mut types = struct_subst(struct_decls, struct_targs);
        let mut values = HashMap::new();
        for (decl, targ) in struct_decls.iter().zip(struct_targs) {
            if let (ParamDecl::Value { name, .. }, TyArg::Val(value)) = (decl, targ) {
                values.insert(name.clone(), value.clone());
            }
        }
        // The member's own explicit parameters concretize from the application.
        // Origin parameters are keyed by the id assigned while the template was
        // lowered (their position in `enclosing_type_params`).
        let mut origins = HashMap::new();
        let mut supplied = args.iter();
        for (index, param) in member.params.iter().enumerate() {
            if param.infer_only {
                continue;
            }
            let Some(arg) = supplied.next() else { break };
            match arg {
                TyArg::Ty(ty) => {
                    types.insert(param.name.clone(), ty.clone());
                }
                TyArg::Val(value) => {
                    values.insert(param.name.clone(), value.clone());
                }
                TyArg::Origin(origin) => {
                    origins.insert((member.param_base + index) as u32, origin.clone());
                }
            }
        }
        let bindings = AssocBindings {
            types,
            values,
            origins,
        };
        Ok(self.resolve_assoc_ty(&substitute_assoc(&member.template, &bindings)))
    }

    pub(super) fn resolve_assoc_ty(&self, ty: &Ty) -> Ty {
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
                    .map(|bound| Box::new(self.resolve_assoc_ty(bound))),
            },
            Ty::Assoc { base, name, args } => {
                let base = self.resolve_assoc_ty(base);
                let args = map_tyargs(args, |t| self.resolve_assoc_ty(t));
                self.associated_type_from_base(&base, name, &args)
                    .unwrap_or_else(|_| Ty::Assoc {
                        base: Box::new(base),
                        name: name.clone(),
                        args,
                    })
            }
            Ty::Struct(name, args) => {
                Ty::Struct(name.clone(), map_tyargs(args, |t| self.resolve_assoc_ty(t)))
            }
            Ty::ComptimeList(elem) => Ty::ComptimeList(Box::new(self.resolve_assoc_ty(elem))),
            Ty::Tuple(elems) => Ty::Tuple(elems.iter().map(|t| self.resolve_assoc_ty(t)).collect()),
            Ty::RuntimePack(elems) => {
                Ty::RuntimePack(elems.iter().map(|t| self.resolve_assoc_ty(t)).collect())
            }
            Ty::Variant(alternatives) => Ty::Variant(
                alternatives
                    .iter()
                    .map(|ty| self.resolve_assoc_ty(ty))
                    .collect(),
            ),
            Ty::Pointer { element, origin } => Ty::Pointer {
                element: Box::new(self.resolve_assoc_ty(element)),
                origin: origin.clone(),
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
                params: params.iter().map(|p| self.resolve_assoc_ty(p)).collect(),
                names: names.clone(),
                ret: Box::new(self.resolve_assoc_ty(ret)),
                required: required.clone(),
                variadic: variadic
                    .as_ref()
                    .map(|v| Box::new(self.resolve_assoc_ty(v))),
                kw_variadic: kw_variadic
                    .as_ref()
                    .map(|v| Box::new(self.resolve_assoc_ty(v))),
                positional_only: *positional_only,
                keyword_only: *keyword_only,
                raises: *raises,
                error: error
                    .as_ref()
                    .map(|error| Box::new(self.resolve_assoc_ty(error))),
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
            } => Ty::GenericFunc {
                environment: environment.clone(),
                decls: decls.clone(),
                params: params.iter().map(|p| self.resolve_assoc_ty(p)).collect(),
                names: names.clone(),
                ret: Box::new(self.resolve_assoc_ty(ret)),
                required: required.clone(),
                variadic: variadic
                    .as_ref()
                    .map(|v| Box::new(self.resolve_assoc_ty(v))),
                kw_variadic: kw_variadic
                    .as_ref()
                    .map(|v| Box::new(self.resolve_assoc_ty(v))),
                positional_only: *positional_only,
                keyword_only: *keyword_only,
                raises: *raises,
                error: error
                    .as_ref()
                    .map(|error| Box::new(self.resolve_assoc_ty(error))),
                conventions: conventions.clone(),
                ref_params: ref_params.clone(),
                ref_return: ref_return.clone(),
            },
            Ty::Overload(candidates) => Ty::Overload(
                candidates
                    .iter()
                    .map(|candidate| self.resolve_assoc_ty(candidate))
                    .collect(),
            ),
            _ => ty.clone(),
        }
    }

    /// Resolve one supplied parameter argument against its declared parameter: a
    /// type parameter takes a type (bound-checked); a value parameter takes a
    /// comptime `Int`. A lone-identifier value argument is reinterpreted as a
    /// type when the parameter is a type parameter.
    pub(super) fn resolve_param_arg(
        &self,
        decl: &ParamDecl,
        arg: &crate::ast::ParamArg,
    ) -> Result<TyArg, TypeError> {
        use crate::ast::ParamArg;
        match decl {
            ParamDecl::Type { name, bounds, .. } => {
                let ty = match arg {
                    ParamArg::Type(t) => self.ty_from_anno(t)?,
                    ParamArg::Value(
                        expression @ Expr {
                            kind: ExprKind::Identifier(id),
                            ..
                        },
                    ) => {
                        // The parser encodes a bare type-argument identifier
                        // as a value expression; once it resolves as a type,
                        // MIR must not emit it as a runtime value register.
                        self.operation_adjustments.borrow_mut().insert(
                            expression.source_span(),
                            crate::checked::SemanticAdjustment::EraseCompileTimeArgument,
                        );
                        self.ty_from_anno(&SourceType::Named(id.clone(), vec![]))?
                    }
                    ParamArg::Value(_) => {
                        return Err(TypeError::TypeMismatch {
                            expected: "a type".to_string(),
                            found: "a value".to_string(),
                            context: format!("type parameter '{}'", name),
                        });
                    }
                    ParamArg::Named { value, .. } => {
                        return self.resolve_param_arg(decl, value);
                    }
                };
                for bound in bounds {
                    if !self.conforms_to(&ty, bound) {
                        return Err(TypeError::TraitNotSatisfied {
                            param: name.clone(),
                            ty: ty.to_string(),
                            trait_name: bound.clone(),
                            reason: self.trait_failure_reason(&ty, bound),
                        });
                    }
                }
                Ok(TyArg::Ty(ty))
            }
            ParamDecl::Value { name, ty, .. } => match arg {
                ParamArg::Value(expr) => {
                    // Function values are compile-time parameters in source, but
                    // deliberately remain runtime values in the VM ABI: MIR
                    // evaluates the parameter argument into a register and the
                    // call frame reifies it under `name`. `CtValue::Param` is only
                    // the erased generic-identity marker used by this resolver.
                    if matches!(ty.as_ref(), Ty::Func { .. } | Ty::GenericFunc { .. }) {
                        let actual = self.infer(expr)?;
                        if !self.value_coerces(&actual, ty) {
                            return Err(TypeError::TypeMismatch {
                                expected: ty.to_string(),
                                found: actual.to_string(),
                                context: format!("callable-value parameter '{}'", name),
                            });
                        }
                        return Ok(TyArg::Val(CtValue::Param(name.clone())));
                    }
                    let value = self.eval_associated_ct(expr, &HashMap::new())?;
                    let actual =
                        self.ct_value_ty(&value, ty)
                            .ok_or_else(|| TypeError::TypeMismatch {
                                expected: ty.to_string(),
                                found: "a non-materializable compile-time value".to_string(),
                                context: format!("value parameter '{}'", name),
                            })?;
                    if !coerces(&actual, ty) {
                        return Err(TypeError::TypeMismatch {
                            expected: ty.to_string(),
                            found: actual.to_string(),
                            context: format!("value parameter '{}'", name),
                        });
                    }
                    self.record_literal_materializations(expr, &actual, ty)?;
                    let rendered = value.to_string();
                    let value = value.clone().materialize_as(ty).ok_or_else(|| {
                        TypeError::TypeMismatch {
                            expected: ty.to_string(),
                            found: rendered,
                            context: format!("value parameter '{}'", name),
                        }
                    })?;
                    Ok(TyArg::Val(value))
                }
                ParamArg::Type(_) => Err(TypeError::TypeMismatch {
                    expected: "a value".to_string(),
                    found: "a type".to_string(),
                    context: format!("value parameter '{}'", name),
                }),
                ParamArg::Named { value, .. } => self.resolve_param_arg(decl, value),
            },
        }
    }

    /// Resolve `List[T]` from its single type argument.
    pub(super) fn list_type(&self, args: &[crate::ast::ParamArg]) -> Result<Ty, TypeError> {
        if args.is_empty() {
            return Ok(list_type(Ty::Infer));
        }
        if args.len() != 1 {
            return Err(TypeError::WrongTypeArgCount {
                name: "List".to_string(),
                expected: 1,
                got: args.len(),
            });
        }
        match &args[0] {
            crate::ast::ParamArg::Type(t) => Ok(list_type(self.ty_from_anno(t)?)),
            // A bare-identifier arg is reinterpreted as a type (as elsewhere).
            crate::ast::ParamArg::Value(Expr {
                kind: ExprKind::Identifier(id),
                ..
            }) => Ok(list_type(
                self.ty_from_anno(&SourceType::Named(id.clone(), vec![]))?,
            )),
            crate::ast::ParamArg::Value(_) => Err(TypeError::TypeMismatch {
                expected: "a type".to_string(),
                found: "a value".to_string(),
                context: "List element type".to_string(),
            }),
            crate::ast::ParamArg::Named { .. } => Err(TypeError::TypeMismatch {
                expected: "a positional type argument".to_string(),
                found: "a named argument".to_string(),
                context: "List element type".to_string(),
            }),
        }
    }

    pub(super) fn collection_type_argument(
        &self,
        collection: &str,
        argument: &crate::ast::ParamArg,
    ) -> Result<Ty, TypeError> {
        match argument {
            crate::ast::ParamArg::Type(ty) => self.ty_from_anno(ty),
            crate::ast::ParamArg::Value(Expr {
                kind: ExprKind::Identifier(name),
                ..
            }) => self.ty_from_anno(&SourceType::Named(name.clone(), Vec::new())),
            crate::ast::ParamArg::Value(_) => Err(TypeError::TypeMismatch {
                expected: "a type".to_string(),
                found: "a value".to_string(),
                context: format!("{collection} type argument"),
            }),
            crate::ast::ParamArg::Named { .. } => Err(TypeError::TypeMismatch {
                expected: "a positional type argument".to_string(),
                found: "a named argument".to_string(),
                context: format!("{collection} type argument"),
            }),
        }
    }

    pub(super) fn set_type(&self, args: &[crate::ast::ParamArg]) -> Result<Ty, TypeError> {
        if args.is_empty() {
            return Ok(set_type(Ty::Infer));
        }
        if args.len() != 1 {
            return Err(TypeError::WrongTypeArgCount {
                name: "Set".to_string(),
                expected: 1,
                got: args.len(),
            });
        }
        Ok(set_type(self.collection_type_argument("Set", &args[0])?))
    }

    pub(super) fn dict_type(&self, args: &[crate::ast::ParamArg]) -> Result<Ty, TypeError> {
        if args.is_empty() {
            return Ok(dict_type(Ty::Infer, Ty::Infer));
        }
        if args.len() != 2 {
            return Err(TypeError::WrongTypeArgCount {
                name: "Dict".to_string(),
                expected: 2,
                got: args.len(),
            });
        }
        Ok(dict_type(
            self.collection_type_argument("Dict", &args[0])?,
            self.collection_type_argument("Dict", &args[1])?,
        ))
    }

    /// Resolve Mojito's legacy `UnsafePointer[T]` spelling or current Mojo's
    /// origin-bearing `UnsafePointer[T, origin]`.  The inferred mutability
    /// parameter is intentionally absent from the user-facing argument list.
    pub(super) fn pointer_type(&self, args: &[crate::ast::ParamArg]) -> Result<Ty, TypeError> {
        if !matches!(args.len(), 1 | 2) {
            return Err(TypeError::WrongTypeArgCount {
                name: "UnsafePointer".to_string(),
                expected: 2,
                got: args.len(),
            });
        }
        let elem = match &args[0] {
            crate::ast::ParamArg::Type(t) => self.ty_from_anno(t)?,
            crate::ast::ParamArg::Value(Expr {
                kind: ExprKind::Identifier(id),
                ..
            }) => self.ty_from_anno(&SourceType::Named(id.clone(), vec![]))?,
            crate::ast::ParamArg::Value(_) => {
                return Err(TypeError::TypeMismatch {
                    expected: "a type".to_string(),
                    found: "a value".to_string(),
                    context: "UnsafePointer element type".to_string(),
                });
            }
            crate::ast::ParamArg::Named { .. } => {
                return Err(TypeError::Unsupported(
                    "named Tuple element arguments".to_string(),
                ));
            }
        };
        let origin = if args.len() == 1 {
            crate::origin::PointerOrigin::Legacy
        } else {
            self.pointer_origin_arg(&args[1])?
        };
        Ok(Ty::Pointer {
            element: Box::new(elem),
            origin,
        })
    }

    pub(super) fn pointer_origin_arg(
        &self,
        argument: &crate::ast::ParamArg,
    ) -> Result<crate::origin::PointerOrigin, TypeError> {
        use crate::origin::{Mutability, OriginParamId, PointerOrigin};

        let constant = match argument {
            crate::ast::ParamArg::Type(SourceType::SelfParam(name)) => {
                let (index, parameter) = self
                    .enclosing_type_params
                    .iter()
                    .enumerate()
                    .find(|(_, parameter)| {
                        parameter.name == *name && parameter.bounds.as_slice() == ["Origin"]
                    })
                    .ok_or_else(|| TypeError::UnknownSelfParam(name.clone()))?;
                let id = OriginParamId(index as u32);
                let mutability = match parameter.origin_mutability.as_ref().map(|e| &e.kind) {
                    Some(ExprKind::Bool(true)) => Mutability::Mutable,
                    Some(ExprKind::Bool(false)) => Mutability::Immutable,
                    _ => Mutability::Param(id),
                };
                return Ok(PointerOrigin::Param { id, mutability });
            }
            crate::ast::ParamArg::Value(Expr {
                kind: ExprKind::Identifier(name),
                ..
            }) => name.as_str(),
            crate::ast::ParamArg::Type(SourceType::Named(name, arguments))
                if arguments.is_empty() =>
            {
                name.as_str()
            }
            _ => {
                return Err(TypeError::TypeMismatch {
                    expected: "Self.origin or a concrete Origin value".to_string(),
                    found: "a non-origin parameter argument".to_string(),
                    context: "UnsafePointer origin".to_string(),
                });
            }
        };
        match constant {
            "MutUntrackedOrigin" => Ok(PointerOrigin::Untracked { mutable: true }),
            "ImmutUntrackedOrigin" => Ok(PointerOrigin::Untracked { mutable: false }),
            "MutUnsafeAnyOrigin" => Ok(PointerOrigin::UnsafeAny { mutable: true }),
            "ImmutUnsafeAnyOrigin" => Ok(PointerOrigin::UnsafeAny { mutable: false }),
            "StaticConstantOrigin" => Ok(PointerOrigin::Static),
            name => Err(TypeError::UndefinedVariable(name.to_string())),
        }
    }

    /// Resolve `Tuple[T1, …, Tn]` from its type arguments (each a type).
    pub(super) fn tuple_element_types(
        &self,
        args: &[crate::ast::ParamArg],
    ) -> Result<Vec<Ty>, TypeError> {
        let mut elems = Vec::with_capacity(args.len());
        for arg in args {
            elems.push(match arg {
                crate::ast::ParamArg::Type(t) => self.ty_from_anno(t)?,
                // A bare-identifier arg is reinterpreted as a type (as elsewhere).
                crate::ast::ParamArg::Value(Expr {
                    kind: ExprKind::Identifier(id),
                    ..
                }) => self.ty_from_anno(&SourceType::Named(id.clone(), vec![]))?,
                crate::ast::ParamArg::Value(_) => {
                    return Err(TypeError::TypeMismatch {
                        expected: "a type".to_string(),
                        found: "a value".to_string(),
                        context: "Tuple element type".to_string(),
                    });
                }
                crate::ast::ParamArg::Named { .. } => {
                    return Err(TypeError::Unsupported(
                        "named Tuple element arguments".to_string(),
                    ));
                }
            });
        }
        Ok(elems)
    }

    pub(super) fn tuple_type(&self, args: &[crate::ast::ParamArg]) -> Result<Ty, TypeError> {
        self.tuple_element_types(args)
            .map(|elements| self.public_tuple_type(elements))
    }

    /// Recover the concrete public-Tuple arguments deliberately materialized by
    /// variadic-struct specialization. A user declaration cannot forge the
    /// compiler-generated symbol because `$` is not a source identifier, and
    /// the canonical symbol is recomputed from the semantic element types rather
    /// than decoded from text.
    pub(super) fn generated_tuple_arguments(
        &self,
        name: &str,
        associated: &[StructComptime],
    ) -> Result<Option<Vec<TyArg>>, TypeError> {
        let Some(element_types) = associated
            .iter()
            .find(|member| member.name == "element_types")
        else {
            return Ok(None);
        };
        let ExprKind::TupleLit(elements) = &element_types.value.kind else {
            return Ok(None);
        };
        let semantic = elements
            .iter()
            .map(|element| match &element.kind {
                ExprKind::TypeValue(ty) => self.ty_from_anno(ty),
                _ => Err(TypeError::NotComptime(
                    "Tuple.element_types must contain only types".to_string(),
                )),
            })
            .collect::<Result<Vec<_>, _>>()?;
        if crate::comptime::tuple_specialization_symbol(&semantic) != name {
            return Ok(None);
        }
        Ok(Some(semantic.into_iter().map(TyArg::Ty).collect()))
    }

    /// Construct the checked identity of an ordinary struct or of a concrete
    /// erased specialization whose source parameters have become fixed facts.
    pub(super) fn struct_instance_type(&self, name: &str, arguments: Vec<TyArg>) -> Ty {
        let arguments = self
            .structs
            .get(name)
            .and_then(|info| info.fixed_arguments.clone())
            .unwrap_or(arguments);
        Ty::Struct(name.to_string(), arguments)
    }

    /// Resolve the alternatives of `Variant[T1, ..., Tn]`.  Alternative order
    /// is significant because it becomes the runtime tag; duplicate types would
    /// make `isa[T]` and `value[T]` ambiguous and are rejected here.
    pub(super) fn variant_type(&self, args: &[crate::ast::ParamArg]) -> Result<Ty, TypeError> {
        if args.is_empty() {
            return Err(TypeError::WrongTypeArgCount {
                name: "Variant".to_string(),
                expected: 1,
                got: 0,
            });
        }
        let mut alternatives = Vec::with_capacity(args.len());
        for arg in args {
            let alternative = self.type_param_argument(arg, "Variant alternative")?;
            if alternatives.contains(&alternative) {
                return Err(TypeError::Unsupported(format!(
                    "Variant contains duplicate alternative '{alternative}'"
                )));
            }
            alternatives.push(alternative);
        }
        Ok(Ty::Variant(alternatives))
    }

    pub(super) fn type_param_argument(
        &self,
        arg: &crate::ast::ParamArg,
        context: &str,
    ) -> Result<Ty, TypeError> {
        match arg {
            crate::ast::ParamArg::Type(ty) => self.ty_from_anno(ty),
            crate::ast::ParamArg::Value(Expr {
                kind: ExprKind::Identifier(name),
                ..
            }) => self.ty_from_anno(&SourceType::Named(name.clone(), Vec::new())),
            crate::ast::ParamArg::Value(_) => Err(TypeError::TypeMismatch {
                expected: "a type".to_string(),
                found: "a value".to_string(),
                context: context.to_string(),
            }),
            crate::ast::ParamArg::Named { .. } => Err(TypeError::Unsupported(format!(
                "named arguments are not supported in {context}"
            ))),
        }
    }

    /// Resolve `SIMD[DType.<dt>, width]` from its two parameter arguments to its
    /// `(dtype, width)` (raw — not canonicalized).
    pub(super) fn simd_dims(
        &self,
        args: &[crate::ast::ParamArg],
    ) -> Result<(Dtype, i64), TypeError> {
        if args.len() != 2 {
            return Err(TypeError::WrongTypeArgCount {
                name: "SIMD".to_string(),
                expected: 2,
                got: args.len(),
            });
        }
        let dtype = dtype_from_arg(&args[0])?;
        let width = if matches!(
            &args[1],
            crate::ast::ParamArg::Value(Expr { kind: ExprKind::Identifier(name), .. }) if name == "_"
        ) {
            -1
        } else {
            self.simd_width(&args[1])?
        };
        Ok((dtype, width))
    }

    /// The (canonicalized) `Ty` for `SIMD[DType.<dt>, width]` — a width-1 `float64`
    /// resolves to `Ty::Float64` (the unification).
    pub(super) fn simd_type(&self, args: &[crate::ast::ParamArg]) -> Result<Ty, TypeError> {
        let (dtype, width) = self.simd_dims(args)?;
        Ok(simd_ty(dtype, width))
    }

    /// Evaluate a SIMD width argument: a comptime `Int` that is a power of two.
    pub(super) fn simd_width(&self, arg: &crate::ast::ParamArg) -> Result<i64, TypeError> {
        let w = match arg {
            crate::ast::ParamArg::Value(expr) => {
                let value = self.eval_ct(expr)?;
                value
                    .to_i64()
                    .ok_or_else(|| TypeError::BadSimdWidth(value.to_string()))?
            }
            crate::ast::ParamArg::Type(_) => {
                return Err(TypeError::BadSimdWidth("a type".to_string()));
            }
            crate::ast::ParamArg::Named { .. } => {
                return Err(TypeError::BadSimdWidth("a named argument".to_string()));
            }
        };
        if w >= 1 && (w & (w - 1)) == 0 {
            Ok(w)
        } else {
            Err(TypeError::BadSimdWidth(w.to_string()))
        }
    }

    /// If `name` is a generic type parameter currently in scope, return its
    /// complete checked type-parameter fact.
    pub(super) fn lookup_tparam(&self, name: &str) -> Option<Ty> {
        self.tparams
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .cloned()
    }
}

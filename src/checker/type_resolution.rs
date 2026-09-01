//! Resolution of source type annotations into checked `Ty` values:
//! type-argument-bearing builtins (List/Set/Dict/Pointer/Tuple/SIMD/Variant),
//! dependent/associated type projection, and type-parameter lookup.
//! Extracted from `checker.rs`; see `docs/symbol-map.md`.

use super::*;

/// Current Mojo treats a bare `def(...)` in a general type position (struct
/// field, collection element) as a trait, not a storable callable-value type;
/// callable values are limited to parameters and local bindings.
pub(super) fn reject_stored_callable_type(ty: &Ty, position: &str) -> Result<(), TypeError> {
    if matches!(ty, Ty::Func { .. } | Ty::GenericFunc { .. }) {
        return Err(TypeError::Unsupported(format!(
            "a 'def(...)' type names a trait in {position} in current Mojo, not a storable \
             callable value; callable values are limited to parameters and 'def(...) thin' \
             local bindings"
        )));
    }
    Ok(())
}

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
            SourceType::StringLiteral => Ty::StringLiteral,
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
                where_clauses,
            } => {
                // Clauses are compiled onto the anonymous contract's decls by
                // `lower_anonymous_callable_type`, which strips them before
                // resolving here. Any clause still present is on a plain
                // function-type annotation, where it has no binder to
                // constrain.
                if !where_clauses.is_empty() {
                    return Err(TypeError::Unsupported(
                        "a 'where' clause on a function type is only supported on a \
                         compile-time callable parameter bound with def[...] parameters"
                            .to_string(),
                    ));
                }
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
                        kind: parameter.kind,
                        convention: parameter.convention,
                        origin: parameter.origin.clone(),
                    })
                    .collect();
                let regular: Vec<&FnParam> = function_params
                    .iter()
                    .filter(|parameter| parameter.kind == crate::ast::ParamKind::Regular)
                    .collect();
                let parameter_types = regular
                    .iter()
                    .map(|parameter| self.resolve_ty_from_anno(&parameter.ty))
                    .collect::<Result<Vec<_>, _>>()?;
                let variadic = function_params
                    .iter()
                    .find(|parameter| parameter.kind == crate::ast::ParamKind::Variadic)
                    .map(|parameter| self.resolve_ty_from_anno(&parameter.ty).map(Box::new))
                    .transpose()?;
                let kw_variadic = function_params
                    .iter()
                    .find(|parameter| parameter.kind == crate::ast::ParamKind::KwVariadic)
                    .map(|parameter| self.resolve_ty_from_anno(&parameter.ty).map(Box::new))
                    .transpose()?;
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
                        .filter(|parameter| parameter.kind == crate::ast::ParamKind::Regular)
                        .map(|parameter| parameter.name.clone())
                        .collect(),
                    ret: Box::new(return_type),
                    required: vec![true; regular.len()],
                    variadic,
                    kw_variadic,
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
                        .filter(|parameter| parameter.kind == crate::ast::ParamKind::Regular)
                        .map(|parameter| parameter.convention)
                        .collect(),
                    ref_params: Box::new(
                        self.lower_callable_ref_param_sigs(type_params, &regular)?,
                    ),
                    ref_return,
                    transfers: Default::default(),
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
                let Some(origin_name) = super::origins::origin_binder_name(origin_expr) else {
                    return Err(TypeError::Unsupported(
                        "reference-valued fields require a named origin parameter".to_string(),
                    ));
                };
                let origin_name = origin_name.to_string();
                if origin_name.ends_with("UnsafeAnyOrigin") {
                    return Err(TypeError::Unsupported(
                        "an UnsafeAnyOrigin reference cannot be hidden in a stored reference                          field"
                            .to_string(),
                    ));
                }
                if origin_name == "ImmUntrackedOrigin" {
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
                {
                    let trait_name = crate::ast::canonical_trait_name(trait_name);
                    if BUILTIN_TRAITS.contains(&trait_name) || self.traits.contains_key(trait_name)
                    {
                        return Ok(Ty::Param {
                            name: format!("Some[{trait_name}]"),
                            bounds: vec![trait_name.to_string()],
                            callable_bound: None,
                        });
                    }
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
                // Upstream compatibility alias: `StringSlice` resolves to the
                // canonical `StringSpan` view (never emitted; diagnostics and
                // display always spell `StringSpan`).
                if name == "StringSlice" && self.structs.contains_key("StringSpan") {
                    return self
                        .ty_from_anno(&SourceType::Named("StringSpan".to_string(), args.clone()));
                }
                // Mojo exposes the compile-time `StringLiteral` type. Mojito
                // materializes string literals directly as runtime strings, so
                // it is represented by the existing string type.
                if name == "StringLiteral" && args.is_empty() {
                    return Ok(Ty::StringLiteral);
                }
                if args.is_empty()
                    && let Some(parameter) = self.lookup_tparam(name)
                {
                    return Ok(parameter);
                }
                // Annotations parse the primitive spellings to dedicated
                // `SourceType` variants; a `Named` primitive reaches this arm
                // only from an expression-derived type position (a comptime
                // alias or associated-member body).
                if args.is_empty() {
                    match name.as_str() {
                        "Int" => return Ok(Ty::Int),
                        "UInt" => return Ok(Ty::UInt),
                        "Bool" => return Ok(Ty::Bool),
                        "Float64" => return Ok(Ty::Float64),
                        _ => {}
                    }
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
                // Compiler-private inline uninit storage (`MaybeUninit`'s
                // field), reachable only from the bundled crossing module.
                if name == crate::types::UNINIT_STORAGE_TYPE_NAME {
                    if !self.bundled_stdlib_declaration {
                        return Err(TypeError::Unsupported(format!(
                            "'{name}' is compiler-private storage; use MaybeUninit from std.memory"
                        )));
                    }
                    if args.len() != 1 {
                        return Err(TypeError::WrongTypeArgCount {
                            name: name.clone(),
                            expected: 1,
                            got: args.len(),
                        });
                    }
                    let element = self.tuple_element_types(args)?.remove(0);
                    return Ok(Ty::Struct(
                        name.clone(),
                        vec![crate::types::TyArg::Ty(element)],
                    ));
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
                // Generic comptime aliases share the redeclaration-checked
                // type namespace with structs; expand an application into the
                // aliased type before the struct lookup.
                if let Some(alias) = self.comptime_aliases.get(name) {
                    let alias = alias.clone();
                    return self.resolve_comptime_alias(name, &alias, args);
                }
                // Relative package re-exports may retain only the sibling
                // module tail in a qualified comptime-alias reference. Resolve
                // that spelling when it selects one unique registered alias.
                if name.starts_with("__module$") {
                    let leaf = name.rsplit('$').next().unwrap_or(name);
                    let mut matches = self
                        .comptime_aliases
                        .iter()
                        .filter(|(candidate, _)| candidate.rsplit('$').next() == Some(leaf));
                    if let Some((candidate, alias)) = matches.next()
                        && matches.next().is_none()
                    {
                        let candidate = candidate.clone();
                        let alias = alias.clone();
                        return self.resolve_comptime_alias(&candidate, &alias, args);
                    }
                }
                if let Some(info) = self.structs.get(name) {
                    let decls = info.decls.clone();
                    let source_params = info.source_params.clone();
                    let (_, tyargs) =
                        self.resolve_struct_use_args(name, &decls, &source_params, args, &[], &[])?;
                    return Ok(self.struct_instance_type(name, tyargs));
                }
                if self.allow_generated_tuple_forward_types && self.declared_structs.contains(name)
                {
                    return self.generated_tuple_forward_type(name, args);
                }
                if matches!(
                    name.as_str(),
                    "UnsafePointer" | "Pointer" | "MutPointer" | "ImmPointer"
                ) {
                    return self.pointer_type(name, args);
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
            // Value-parameterized structs carry their symbolic `CtValue::Param`
            // arguments here; specialization bakes them out.
            SourceType::SelfType => match &self.self_ty {
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

    /// Resolve a struct application's explicit compile-time arguments,
    /// accepting origin arguments in the slots the declaration's raw
    /// parameter list spells (`decls` erases Origin parameters, so
    /// `resolve_use_params` alone cannot see them). Origin arguments are
    /// validated and erased — struct identity stays origin-free — and the
    /// remaining arguments forward unchanged. For compatibility, an
    /// application supplying exactly the non-origin explicit count omits the
    /// origin slots entirely.
    pub(super) fn resolve_struct_use_args(
        &self,
        name: &str,
        decls: &[ParamDecl],
        source_params: &[crate::ast::TypeParam],
        args: &[crate::ast::ParamArg],
        patterns: &[Ty],
        actuals: &[Ty],
    ) -> Result<(HashMap<String, Ty>, Vec<TyArg>), TypeError> {
        use crate::ast::ParamArg;
        let is_origin = |p: &crate::ast::TypeParam| matches!(p.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet");
        let explicit: Vec<&crate::ast::TypeParam> =
            source_params.iter().filter(|p| !p.infer_only).collect();
        let origin_slots = explicit.iter().filter(|p| is_origin(p)).count();
        let strict = self.strict_storage_annotation.get();
        // A storage annotation must bind explicit origin slots. A bare name
        // is not concrete unless an initializer can infer the whole
        // parameter list (`AllowBare`); a partial application names the
        // first omitted slot in either strict mode.
        if strict == super::StorageStrictness::Full && origin_slots > 0 && args.is_empty() {
            return Err(TypeError::NotConcrete(name.to_string()));
        }
        // No origin slots — or a variadic explicit list, whose positional
        // alignment the erased-decl binder owns — resolves as before.
        if origin_slots == 0 || args.is_empty() || explicit.iter().any(|p| p.name.starts_with('*'))
        {
            return self.resolve_use_params(name, decls, args, patterns, actuals);
        }
        let any_named = args.iter().any(|a| matches!(a, ParamArg::Named { .. }));
        if !any_named {
            let non_origin = explicit.len() - origin_slots;
            if args.len() == non_origin {
                if strict != super::StorageStrictness::Off {
                    let omitted = explicit
                        .iter()
                        .find(|param| is_origin(param))
                        .expect("origin_slots > 0");
                    return Err(TypeError::CannotInferParam {
                        name: name.to_string(),
                        param: omitted.name.clone(),
                    });
                }
                return self.resolve_use_params(name, decls, args, patterns, actuals);
            }
            if args.len() != explicit.len() {
                return Err(TypeError::WrongTypeArgCount {
                    name: name.to_string(),
                    expected: explicit.len(),
                    got: args.len(),
                });
            }
            // Full positional supply. Every origin slot's argument must
            // RESOLVE as an origin for this interpretation to hold; a
            // non-origin argument in an origin slot means the application is
            // an ordinary over-application (e.g. an infer-only binder spelled
            // explicitly), which the erased-decl binder diagnoses.
            let mut resolved = Vec::with_capacity(origin_slots);
            for (param, argument) in explicit.iter().zip(args) {
                if !is_origin(param) {
                    continue;
                }
                // Upstream's placeholder spellings (`_`, `...`) mark the slot
                // explicitly inferred: the application is complete (no
                // partial-application rejection), the slot resolves from the
                // initializer or call context, and concrete-storage positions
                // reject exactly like an omitted slot.
                if origin_placeholder(argument) {
                    if strict == super::StorageStrictness::Full {
                        return Err(TypeError::NotConcrete(name.to_string()));
                    }
                    self.accept_origin_argument(name, param, argument, None)?;
                    continue;
                }
                match self.resolve_origin_param_arg(argument) {
                    Ok(origin) => resolved.push((*param, argument, origin)),
                    Err(_) => {
                        // A signature annotation may name places only the body
                        // can resolve (`origin_of(self.entries)`): accept an
                        // origin-shaped argument syntactically and erase it.
                        if self.signature_origin_leniency.get()
                            && syntactic_origin_argument(argument)
                        {
                            self.accept_origin_argument(name, param, argument, None)?;
                            continue;
                        }
                        return self.resolve_use_params(name, decls, args, patterns, actuals);
                    }
                }
            }
            for (param, argument, (_, mutability)) in resolved {
                self.accept_origin_argument(name, param, argument, mutability)?;
            }
            let forwarded: Vec<crate::ast::ParamArg> = explicit
                .iter()
                .zip(args)
                .filter(|(param, _)| !is_origin(param))
                .map(|(_, argument)| argument.clone())
                .collect();
            return self.resolve_use_params(name, decls, &forwarded, patterns, actuals);
        }
        // Keyword spellings: extract named origin arguments wherever they
        // appear; everything else forwards to the erased-decl binder.
        let mut forwarded = Vec::with_capacity(args.len());
        let mut supplied_origins: Vec<&str> = Vec::new();
        for argument in args {
            if let ParamArg::Named { name: keyword, .. } = argument
                && let Some(param) = explicit.iter().find(|p| p.name == *keyword && is_origin(p))
            {
                if origin_placeholder(argument) {
                    if strict == super::StorageStrictness::Full {
                        return Err(TypeError::NotConcrete(name.to_string()));
                    }
                    self.accept_origin_argument(name, param, argument, None)?;
                    supplied_origins.push(&param.name);
                    continue;
                }
                match self.resolve_origin_param_arg(argument) {
                    Ok((_, mutability)) => {
                        self.accept_origin_argument(name, param, argument, mutability)?;
                        supplied_origins.push(&param.name);
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            }
            forwarded.push(argument.clone());
        }
        if strict != super::StorageStrictness::Off
            && let Some(omitted) = explicit
                .iter()
                .find(|param| is_origin(param) && !supplied_origins.contains(&param.name.as_str()))
        {
            return Err(TypeError::CannotInferParam {
                name: name.to_string(),
                param: omitted.name.clone(),
            });
        }
        self.resolve_use_params(name, decls, &forwarded, patterns, actuals)
    }

    /// Resolve a return annotation: explicit origin slots must be applied
    /// there (a bare origin-slotted generic or a placeholder is not concrete,
    /// pin-attested), while an applied origin expression naming places only
    /// the body can resolve (`origin_of(self.entries)`) is accepted
    /// syntactically and erased.
    pub(super) fn resolve_return_annotation(
        &self,
        annotation: &crate::ast::SourceType,
    ) -> Result<Ty, TypeError> {
        let saved = self.signature_origin_leniency.replace(true);
        let result = self.resolve_storage_annotation(annotation, super::StorageStrictness::Full);
        self.signature_origin_leniency.set(saved);
        result
    }

    /// Resolve a type annotation in a storage position (a struct field or a
    /// local `var` type): explicit origin slots must be bound there, because
    /// storage has no constructor value argument to infer them from.
    pub(super) fn resolve_storage_annotation(
        &self,
        annotation: &crate::ast::SourceType,
        strictness: super::StorageStrictness,
    ) -> Result<Ty, TypeError> {
        let saved = self.strict_storage_annotation.replace(strictness);
        let result = self.ty_from_anno(annotation);
        self.strict_storage_annotation.set(saved);
        result
    }

    /// Accept one resolved origin argument against its declared slot: a slot
    /// declared `Origin[mut=True]` rejects a provably immutable argument, and
    /// the accepted argument is marked erased — it is a compile-time fact, so
    /// at a constructor expression MIR must not emit it as a runtime value
    /// register.
    fn accept_origin_argument(
        &self,
        struct_name: &str,
        param: &crate::ast::TypeParam,
        argument: &crate::ast::ParamArg,
        mutability: Option<crate::origin::Mutability>,
    ) -> Result<(), TypeError> {
        let requires_mut = matches!(
            param.origin_mutability.as_ref().map(|e| &e.kind),
            Some(ExprKind::Bool(true))
        );
        if requires_mut && matches!(mutability, Some(crate::origin::Mutability::Immutable)) {
            return Err(TypeError::TypeMismatch {
                expected: format!(
                    "a mutable origin for parameter '{}' of '{struct_name}'",
                    param.name
                ),
                found: "an immutable origin".to_string(),
                context: "explicit origin argument".to_string(),
            });
        }
        let mut value = argument;
        while let crate::ast::ParamArg::Named { value: inner, .. } = value {
            value = inner;
        }
        if let crate::ast::ParamArg::Value(expression) = value {
            self.operation_adjustments.borrow_mut().insert(
                expression.source_span(),
                crate::checked::SemanticAdjustment::EraseCompileTimeArgument,
            );
        }
        Ok(())
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
                transfers,
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
                transfers: transfers.clone(),
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
                if let Some(constraints) = info.associated_constraints.get(name) {
                    let environment: HashMap<&str, &TyArg> = info
                        .decls
                        .iter()
                        .zip(targs)
                        .map(|(declaration, argument)| {
                            (declaration.name().trim_start_matches('*'), argument)
                        })
                        .collect();
                    if !environment
                        .values()
                        .any(|argument| tyarg_is_symbolic(argument))
                    {
                        for constraint in constraints {
                            self.validate_constraint_in_environment(
                                &format!("{base}.{name}"),
                                constraint,
                                &environment,
                            )?;
                        }
                    }
                }
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

    /// Expand a generic comptime alias application (`Alias[args]`) into the
    /// aliased type. `resolve_use_params` validates arity, bounds, defaults,
    /// and the alias's declaration constraints — including repeated `where`
    /// clauses, each retaining its own message — through the same contract a
    /// struct application uses; the resulting arguments then substitute into
    /// the symbolic template.
    fn resolve_comptime_alias(
        &self,
        name: &str,
        alias: &ComptimeAlias,
        args: &[crate::ast::ParamArg],
    ) -> Result<Ty, TypeError> {
        let AliasBody::Type(template) = &alias.body else {
            return Err(TypeError::Unsupported(format!(
                "'{name}' is a Bool-valued comptime alias, not a type"
            )));
        };
        let (_, tyargs) = self.resolve_use_params(name, &alias.decls, args, &[], &[])?;
        let mut types = HashMap::new();
        let mut values = HashMap::new();
        for (decl, argument) in alias.decls.iter().zip(&tyargs) {
            match (decl, argument) {
                (ParamDecl::Type { name, .. }, TyArg::Ty(ty)) => {
                    types.insert(name.clone(), ty.clone());
                }
                (ParamDecl::Value { name, .. }, TyArg::Val(value)) => {
                    values.insert(name.clone(), value.clone());
                }
                _ => {}
            }
        }
        let bindings = AssocBindings {
            types,
            values,
            // Origin parameters are rejected at alias declaration.
            origins: HashMap::new(),
        };
        // The symbolic template keeps canonical `Tuple[T, ...]`; a substituted
        // application must re-select the executable nominal implementation so
        // discovery can materialize it.
        Ok(self.canonicalize_public_tuple_types(
            self.resolve_assoc_ty(&substitute_assoc(template, &bindings)),
        ))
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
        if !member.availability.is_empty() {
            let mut environment: HashMap<&str, &TyArg> = struct_decls
                .iter()
                .zip(struct_targs)
                .map(|(declaration, argument)| {
                    (declaration.name().trim_start_matches('*'), argument)
                })
                .collect();
            for (parameter, argument) in member
                .params
                .iter()
                .filter(|parameter| !parameter.infer_only)
                .zip(args)
            {
                environment.insert(parameter.name.trim_start_matches('*'), argument);
            }
            if !environment
                .values()
                .any(|argument| tyarg_is_symbolic(argument))
            {
                for constraint in &member.availability {
                    self.validate_constraint_in_environment(
                        &format!("{base}.{name}"),
                        constraint,
                        &environment,
                    )?;
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
                transfers,
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
                transfers: transfers.clone(),
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
                    // A symbolic enclosing-scope parameter stays symbolic here;
                    // specialization bakes it out like the callable case above.
                    if matches!(value, CtValue::Param(_)) {
                        return Ok(TyArg::Val(value));
                    }
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
        Ok(list_type(self.collection_type_argument("List", &args[0])?))
    }

    pub(super) fn collection_type_argument(
        &self,
        collection: &str,
        argument: &crate::ast::ParamArg,
    ) -> Result<Ty, TypeError> {
        let resolved = match argument {
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
        }?;
        reject_stored_callable_type(&resolved, &format!("the '{collection}' element type"))?;
        Ok(resolved)
    }

    pub(super) fn set_type(&self, args: &[crate::ast::ParamArg]) -> Result<Ty, TypeError> {
        if args.is_empty() {
            return Ok(self.nominal_set(Ty::Infer));
        }
        match args {
            [element] => Ok(self.nominal_set(self.collection_type_argument("Set", element)?)),
            [element, hasher] => Ok(crate::types::set_type_with(
                self.collection_type_argument("Set", element)?,
                self.hasher_type_argument("Set", hasher)?,
            )),
            _ => Err(TypeError::WrongTypeArgCount {
                name: "Set".to_string(),
                expected: 2,
                got: args.len(),
            }),
        }
    }

    pub(super) fn dict_type(&self, args: &[crate::ast::ParamArg]) -> Result<Ty, TypeError> {
        if args.is_empty() {
            return Ok(self.nominal_dict(Ty::Infer, Ty::Infer));
        }
        match args {
            [key, value] => Ok(self.nominal_dict(
                self.collection_type_argument("Dict", key)?,
                self.collection_type_argument("Dict", value)?,
            )),
            [key, value, hasher] => Ok(crate::types::dict_type_with(
                self.collection_type_argument("Dict", key)?,
                self.collection_type_argument("Dict", value)?,
                self.hasher_type_argument("Dict", hasher)?,
            )),
            _ => Err(TypeError::WrongTypeArgCount {
                name: "Dict".to_string(),
                expected: 3,
                got: args.len(),
            }),
        }
    }

    /// `Set[T]` with the hasher argument filled from the linked declaration's
    /// default (`H: Hasher = default_hasher`); a seam program without the
    /// stdlib keeps the element-only spelling.
    pub(super) fn nominal_set(&self, element: Ty) -> Ty {
        match self.default_hasher_argument(crate::types::SET_TYPE_NAME) {
            Some(hasher) => crate::types::set_type_with(element, hasher),
            None => set_type(element),
        }
    }

    /// `Dict[K, V]` with the hasher argument filled from the linked
    /// declaration's default; see [`Self::nominal_set`].
    pub(super) fn nominal_dict(&self, key: Ty, value: Ty) -> Ty {
        match self.default_hasher_argument(crate::types::DICT_TYPE_NAME) {
            Some(hasher) => crate::types::dict_type_with(key, value, hasher),
            None => dict_type(key, value),
        }
    }

    /// The declared default of a hash collection's trailing `H: Hasher`
    /// parameter, when the collection is linked with one.
    fn default_hasher_argument(&self, collection: &str) -> Option<Ty> {
        let info = self.structs.get(collection)?;
        match info.decls.last()? {
            ParamDecl::Type {
                bounds,
                default: Some(default),
                ..
            } if bounds.iter().any(|bound| bound == "Hasher") => Some((**default).clone()),
            _ => None,
        }
    }

    /// Resolve an explicit hasher type argument of a hash collection; it
    /// must name a `Hasher` conformer.
    fn hasher_type_argument(
        &self,
        collection: &str,
        argument: &crate::ast::ParamArg,
    ) -> Result<Ty, TypeError> {
        let hasher = self.collection_type_argument(collection, argument)?;
        if !self.conforms_to(&hasher, "Hasher") {
            return Err(TypeError::TraitNotSatisfied {
                param: "H".to_string(),
                ty: hasher.to_string(),
                trait_name: "Hasher".to_string(),
                reason: self.trait_failure_reason(&hasher, "Hasher"),
            });
        }
        Ok(hasher)
    }

    /// Resolve current Mojo's `Pointer[T, origin]` (also spelled through the
    /// deprecated `UnsafePointer` alias and the mutability-fixing `MutPointer`/
    /// `ImmPointer` aliases) or the one-argument `Pointer[T]` compatibility
    /// spelling, which resolves to the mutable untracked origin of heap
    /// allocations. The inferred mutability parameter is intentionally absent
    /// from the user-facing argument list.
    pub(super) fn pointer_type(
        &self,
        name: &str,
        args: &[crate::ast::ParamArg],
    ) -> Result<Ty, TypeError> {
        if !matches!(args.len(), 1 | 2) {
            return Err(TypeError::WrongTypeArgCount {
                name: name.to_string(),
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
                    context: format!("{name} element type"),
                });
            }
            crate::ast::ParamArg::Named { .. } => {
                return Err(TypeError::Unsupported(
                    "named Tuple element arguments".to_string(),
                ));
            }
        };
        let origin = if args.len() == 1 {
            crate::origin::PointerOrigin::Untracked {
                mutable: name != "ImmPointer",
            }
        } else {
            self.pointer_origin_arg(&args[1])?
        };
        // The mutability-fixing aliases constrain the origin argument's
        // statically known permission.
        let required = match name {
            "MutPointer" => Some(true),
            "ImmPointer" => Some(false),
            _ => None,
        };
        if let (Some(required), Some(actual)) = (required, origin.statically_mutable())
            && required != actual
        {
            return Err(TypeError::TypeMismatch {
                expected: format!(
                    "an origin with {} permission",
                    if required { "mutable" } else { "immutable" }
                ),
                found: format!(
                    "{} origin",
                    if actual { "a mutable" } else { "an immutable" }
                ),
                context: format!("{name} origin"),
            });
        }
        Ok(Ty::Pointer {
            element: Box::new(elem),
            origin,
        })
    }

    pub(super) fn pointer_origin_arg(
        &self,
        argument: &crate::ast::ParamArg,
    ) -> Result<crate::origin::PointerOrigin, TypeError> {
        use crate::origin::PointerOrigin;

        let constant = match argument {
            crate::ast::ParamArg::Type(SourceType::SelfParam(name)) => {
                return self.enclosing_origin_param(name);
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
            crate::ast::ParamArg::Value(expression) => {
                return self.pointer_origin_expr(expression);
            }
            // `origin._subtree` in type-annotation position parses as an
            // associated member of the origin base; expression positions
            // route through `pointer_origin_expr` instead.
            crate::ast::ParamArg::Type(SourceType::Assoc { base, name, args })
                if name == "_subtree" && args.is_empty() =>
            {
                let origin =
                    self.pointer_origin_arg(&crate::ast::ParamArg::Type((**base).clone()))?;
                return append_subtree(origin);
            }
            // `Self.origin._get_owned_interior["tag"]` in type-annotation
            // position parses as an indexed projection over an associated
            // member of the origin parameter; expression positions route
            // through `pointer_origin_expr` instead.
            crate::ast::ParamArg::Type(SourceType::IndexedProjection { base, index }) => {
                let SourceType::Assoc {
                    base: origin_base,
                    name,
                    args,
                } = base.as_ref()
                else {
                    return Err(TypeError::TypeMismatch {
                        expected: "an interior origin projection".to_string(),
                        found: "an indexed type projection".to_string(),
                        context: "Pointer origin".to_string(),
                    });
                };
                if name != "_get_owned_interior" || !args.is_empty() {
                    return Err(TypeError::TypeMismatch {
                        expected: "'_get_owned_interior[\"tag\"]'".to_string(),
                        found: format!("'{name}'"),
                        context: "Pointer origin projection".to_string(),
                    });
                }
                let ExprKind::Str(tag) = &index.kind else {
                    return Err(TypeError::TypeMismatch {
                        expected: "a compile-time tag string".to_string(),
                        found: "a non-string index".to_string(),
                        context: "interior origin projection".to_string(),
                    });
                };
                let origin =
                    self.pointer_origin_arg(&crate::ast::ParamArg::Type((**origin_base).clone()))?;
                return append_interior_tag(origin, tag);
            }
            _ => {
                return Err(TypeError::TypeMismatch {
                    expected: "Self.origin or a concrete Origin value".to_string(),
                    found: "a non-origin parameter argument".to_string(),
                    context: "Pointer origin".to_string(),
                });
            }
        };
        match constant {
            "MutUntrackedOrigin" => Ok(PointerOrigin::Untracked { mutable: true }),
            "ImmUntrackedOrigin" => Ok(PointerOrigin::Untracked { mutable: false }),
            "MutUnsafeAnyOrigin" => Ok(PointerOrigin::UnsafeAny { mutable: true }),
            "ImmUnsafeAnyOrigin" => Ok(PointerOrigin::UnsafeAny { mutable: false }),
            "ImmStaticOrigin" => Ok(PointerOrigin::Static),
            // The pre-rename alias spellings were removed upstream (2026-08);
            // give them targeted migration diagnostics.
            "ImmutUntrackedOrigin" | "ImmutExternalOrigin" => Err(TypeError::Unsupported(format!(
                "'{constant}' was removed; use 'ImmUntrackedOrigin'"
            ))),
            "ImmutUnsafeAnyOrigin" => Err(TypeError::Unsupported(
                "'ImmutUnsafeAnyOrigin' was removed; use 'ImmUnsafeAnyOrigin'".to_string(),
            )),
            "StaticConstantOrigin" => Err(TypeError::Unsupported(
                "'StaticConstantOrigin' was removed; use 'ImmStaticOrigin'".to_string(),
            )),
            "ExternalOrigin" => Err(TypeError::Unsupported(
                "'ExternalOrigin' was removed; use 'UntrackedOrigin'".to_string(),
            )),
            "MutExternalOrigin" => Err(TypeError::Unsupported(
                "'MutExternalOrigin' was removed; use 'MutUntrackedOrigin'".to_string(),
            )),
            name => self
                .enclosing_origin_param(name)
                .map_err(|_| TypeError::UndefinedVariable(name.to_string())),
        }
    }

    /// Resolve an expression-shaped `Pointer` origin argument: an
    /// `._get_owned_interior["tag"]` projection over an origin parameter or
    /// `origin_of(place)` observation. The projected tags become the
    /// pointer's interior-generation domain — the marker that the pointer
    /// legally designates multiple elements.
    fn pointer_origin_expr(
        &self,
        expression: &Expr,
    ) -> Result<crate::origin::PointerOrigin, TypeError> {
        use crate::origin::{Mutability, Origin, PointerOrigin};

        if let Some((base, tag)) = super::origins::interior_origin_syntax(expression) {
            let origin = self.pointer_origin_expr(base)?;
            return append_interior_tag(origin, tag);
        }
        if let Some(base) = super::origins::subtree_origin_syntax(expression) {
            let origin = self.pointer_origin_expr(base)?;
            return append_subtree(origin);
        }
        match &expression.kind {
            // `Self.origin` spelled in expression position (a projection base).
            ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self") => {
                self.enclosing_origin_param(field)
            }
            // A bare origin parameter name (function-head spelling).
            ExprKind::Identifier(name) => self.enclosing_origin_param(name),
            ExprKind::Call {
                name,
                args,
                kwargs,
                param_args,
            } if name == "origin_of"
                && kwargs.is_empty()
                && param_args.is_empty()
                && args.len() == 1 =>
            {
                // `origin_of(self)` stays symbolic whether or not a `self`
                // place is bound, so a declared return annotation and the
                // body's `unsafe_origin_cast` target resolve to one comparable
                // form; call sites rebase it onto the concrete receiver.
                if matches!(&args[0].kind, ExprKind::Identifier(name) if name == "self") {
                    return Ok(PointerOrigin::SelfPlace {
                        mutability: Mutability::Param(crate::origin::OriginParamId(0)),
                        interior: Vec::new(),
                        subtree: false,
                    });
                }
                let reference = self.reference_actual(&args[0])?;
                match reference.origin {
                    Origin::Place(place) => Ok(PointerOrigin::Place {
                        place,
                        mutable: matches!(reference.mutability, Mutability::Mutable),
                    }),
                    Origin::Param(id) => Ok(PointerOrigin::Param {
                        id,
                        mutability: reference.mutability,
                        interior: Vec::new(),
                        subtree: false,
                    }),
                    other => Err(TypeError::Unsupported(format!(
                        "origin_of over {other:?} is not a supported Pointer origin argument"
                    ))),
                }
            }
            _ => Err(TypeError::TypeMismatch {
                expected: "Self.origin, origin_of(place), or a builtin Origin value".to_string(),
                found: "a runtime value".to_string(),
                context: "Pointer origin".to_string(),
            }),
        }
    }

    /// Look up an enclosing `Origin`-bounded type parameter by name and
    /// resolve its declared mutability.
    pub(super) fn enclosing_origin_param(
        &self,
        name: &str,
    ) -> Result<crate::origin::PointerOrigin, TypeError> {
        use crate::origin::{Mutability, OriginParamId, PointerOrigin};

        let (index, parameter) = self
            .enclosing_type_params
            .iter()
            .enumerate()
            .find(|(_, parameter)| {
                parameter.name == *name && parameter.bounds.as_slice() == ["Origin"]
            })
            .ok_or_else(|| TypeError::UnknownSelfParam(name.to_string()))?;
        let id = OriginParamId(index as u32);
        let mutability = match parameter.origin_mutability.as_ref().map(|e| &e.kind) {
            Some(ExprKind::Bool(true)) => Mutability::Mutable,
            Some(ExprKind::Bool(false)) => Mutability::Immutable,
            _ => Mutability::Param(id),
        };
        Ok(PointerOrigin::Param {
            id,
            mutability,
            interior: Vec::new(),
            subtree: false,
        })
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
        let elements = self.tuple_element_types(args)?;
        for element in &elements {
            reject_stored_callable_type(element, "the 'Tuple' element type")?;
        }
        Ok(self.public_tuple_type(elements))
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

    /// The struct's own instance type as `Self` resolves to inside its methods:
    /// `Ty::Struct(name, decls.map(param_as_arg))` — the same value the checker
    /// installs as `self_ty` during registration, so a `self`-typed parameter's
    /// resolved type is equal to it. Used to canonicalize overload keys back to
    /// `Self` (see [`method_lowered_name`]). `None` when `name` is not a
    /// registered struct (e.g. abstract trait dispatch).
    pub(super) fn self_instance_ty(&self, name: &str) -> Option<Ty> {
        // The nominal String struct keeps its stable `String` overload spelling
        // on both sides; canonicalizing it to `Self` would break the
        // literal→String constructor bridge (see `self_struct_spelling`).
        if crate::symbol::is_stdlib_string_struct(name) {
            return None;
        }
        let info = self.structs.get(name)?;
        Some(Ty::Struct(
            name.to_string(),
            info.decls.iter().map(param_as_arg).collect(),
        ))
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
            reject_stored_callable_type(&alternative, "a 'Variant' alternative type")?;
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

fn tyarg_is_symbolic(argument: &TyArg) -> bool {
    match argument {
        TyArg::Val(CtValue::Param(_)) => true,
        TyArg::Val(CtValue::Tuple(values) | CtValue::List(values)) => values
            .iter()
            .any(|value| matches!(value, CtValue::Param(_))),
        TyArg::Val(_) => false,
        TyArg::Origin(crate::origin::Origin::Param(_) | crate::origin::Origin::SelfParam) => true,
        TyArg::Origin(_) => false,
        TyArg::Ty(ty) => type_is_symbolic(ty),
    }
}

fn type_is_symbolic(ty: &Ty) -> bool {
    match ty {
        Ty::Param { .. } | Ty::Assoc { .. } | Ty::Dependent(_) | Ty::SelfType | Ty::Infer => true,
        Ty::Struct(_, arguments) => arguments.iter().any(tyarg_is_symbolic),
        Ty::Simd { .. }
        | Ty::Int
        | Ty::UInt
        | Ty::Bool
        | Ty::StringLiteral
        | Ty::Float64
        | Ty::None
        | Ty::Never
        | Ty::IntLiteral
        | Ty::FloatLiteral
        | Ty::Dtype
        | Ty::Error => false,
        Ty::Func {
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        }
        | Ty::GenericFunc {
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        } => {
            params.iter().any(type_is_symbolic)
                || type_is_symbolic(ret)
                || variadic.as_deref().is_some_and(type_is_symbolic)
                || kw_variadic.as_deref().is_some_and(type_is_symbolic)
                || error.as_deref().is_some_and(type_is_symbolic)
        }
        Ty::Overload(types) | Ty::Tuple(types) | Ty::RuntimePack(types) | Ty::Variant(types) => {
            types.iter().any(type_is_symbolic)
        }
        Ty::ComptimeList(element) | Ty::VariadicPack(element) => type_is_symbolic(element),
        Ty::Pointer { element, .. } => type_is_symbolic(element),
        Ty::Ref(reference) => type_is_symbolic(&reference.referent),
    }
}

/// Append an interior-generation tag to a tracked pointer origin — the
/// resolution of a `._get_owned_interior["tag"]` projection in a `Pointer`
/// origin argument. Untracked provenances have no place to project into.
fn append_interior_tag(
    mut origin: crate::origin::PointerOrigin,
    tag: &str,
) -> Result<crate::origin::PointerOrigin, TypeError> {
    use crate::origin::{OriginSeg, PointerOrigin};
    if origin_has_subtree_tail(&origin) {
        return Err(subtree_is_terminal_error());
    }
    match &mut origin {
        PointerOrigin::Param { interior, .. } | PointerOrigin::SelfPlace { interior, .. } => {
            interior.push(tag.to_string());
        }
        PointerOrigin::Place { place, .. } => {
            place.path.push(OriginSeg::Interior(tag.to_string()));
        }
        _ => {
            return Err(TypeError::TypeMismatch {
                expected: "an origin parameter or origin_of(place) base".to_string(),
                found: "an untracked origin".to_string(),
                context: "interior origin projection".to_string(),
            });
        }
    }
    Ok(origin)
}

/// Append the conservative `._subtree` projection to a tracked pointer origin.
/// Subtree is terminal: nothing projects below it, including another
/// `._subtree`. Untracked provenances have no place to project into.
fn append_subtree(
    mut origin: crate::origin::PointerOrigin,
) -> Result<crate::origin::PointerOrigin, TypeError> {
    use crate::origin::{OriginSeg, PointerOrigin};
    if origin_has_subtree_tail(&origin) {
        return Err(subtree_is_terminal_error());
    }
    match &mut origin {
        PointerOrigin::Param { subtree, .. } | PointerOrigin::SelfPlace { subtree, .. } => {
            *subtree = true;
        }
        PointerOrigin::Place { place, .. } => {
            place.path.push(OriginSeg::Subtree);
        }
        _ => {
            return Err(TypeError::TypeMismatch {
                expected: "an origin parameter or origin_of(place) base".to_string(),
                found: "an untracked origin".to_string(),
                context: "'_subtree' origin projection".to_string(),
            });
        }
    }
    Ok(origin)
}

fn origin_has_subtree_tail(origin: &crate::origin::PointerOrigin) -> bool {
    use crate::origin::{OriginSeg, PointerOrigin};
    match origin {
        PointerOrigin::Param { subtree, .. } | PointerOrigin::SelfPlace { subtree, .. } => *subtree,
        PointerOrigin::Place { place, .. } => {
            matches!(place.path.last(), Some(OriginSeg::Subtree))
        }
        _ => false,
    }
}

fn subtree_is_terminal_error() -> TypeError {
    TypeError::Unsupported(
        "'_subtree' is a terminal origin projection: nothing can be projected below it".to_string(),
    )
}

/// Whether an application argument is upstream's origin placeholder spelling
/// (`_` or `...`), possibly behind a keyword (`origin=_`): the slot is
/// explicitly marked inferred rather than omitted.
fn origin_placeholder(argument: &crate::ast::ParamArg) -> bool {
    let mut value = argument;
    while let crate::ast::ParamArg::Named { value: inner, .. } = value {
        value = inner;
    }
    matches!(
        value,
        crate::ast::ParamArg::Value(Expr {
            kind: ExprKind::Identifier(name),
            ..
        }) if name == "_" || name == "..."
    )
}

/// Whether an application argument is syntactically origin-shaped — an
/// `origin_of(...)` call, a binder name (`o`, `Self.o`), or a projected origin
/// expression — regardless of whether its places resolve in this context.
fn syntactic_origin_argument(argument: &crate::ast::ParamArg) -> bool {
    let mut value = argument;
    while let crate::ast::ParamArg::Named { value: inner, .. } = value {
        value = inner;
    }
    match value {
        crate::ast::ParamArg::Type(crate::ast::Type::Named(_, targs)) => targs.is_empty(),
        crate::ast::ParamArg::Type(crate::ast::Type::SelfParam(_)) => true,
        crate::ast::ParamArg::Value(expression) => {
            matches!(
                &expression.kind,
                ExprKind::Call { name, .. } if name == "origin_of"
            ) || matches!(
                &expression.kind,
                ExprKind::Identifier(_) | ExprKind::Member { .. } | ExprKind::Index { .. }
            )
        }
        _ => false,
    }
}

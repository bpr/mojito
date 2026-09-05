//! Trait and struct declaration checking, trait conformance (nominal and
//! built-in), type-capability queries (Copyable/Movable/Hashable/…), and
//! trait-method/associated-member lookup. Extracted from `checker.rs`;
//! see `docs/symbol-map.md`.

use super::*;

/// How a consuming position takes its value: an ownership **move** into new
/// storage (gated on `Movable`), or a **deinit** binding — consumption by a
/// destructor/named-destructor receiver or `deinit` parameter, which must stay
/// legal for a non-Movable (`Movable where False`) value.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ConsumeKind {
    Move,
    Deinit,
}

impl Checker {
    /// A trait name is valid if it is a built-in or a user trait defined so far.
    pub(super) fn check_trait_name(&self, name: &str) -> Result<(), TypeError> {
        if BUILTIN_TRAITS.contains(&name) || self.traits.contains_key(name) {
            Ok(())
        } else {
            Err(TypeError::UnknownTrait(name.to_string()))
        }
    }

    /// Register and check a `trait`: its method requirements (each typed with
    /// `Self` as the abstract conforming type, `Ty::SelfType`).
    pub(super) fn check_trait(
        &mut self,
        name: &str,
        refines: &[String],
        methods: &[mojito_ast::ast::TraitMethod],
        comptime_members: &[TraitComptime],
    ) -> Result<(), TypeError> {
        if self.traits.contains_key(name) || self.structs.contains_key(name) {
            return Err(TypeError::Redeclaration(name.to_string()));
        }
        for parent in refines {
            self.check_trait_name(parent)?;
            if BUILTIN_TRAITS.contains(&parent.as_str()) {
                return Err(TypeError::Unsupported(format!(
                    "user trait '{name}' cannot refine builtin trait '{parent}' yet"
                )));
            }
        }
        let mut ct_members = HashMap::new();
        let mut ct_constraints = HashMap::new();
        for parent in refines {
            let inherited = self.traits.get(parent).ok_or_else(|| {
                TypeError::InvariantViolation(format!("trait '{parent}' was not registered"))
            })?;
            for (member, requirement) in &inherited.comptime_members {
                if let Some(existing) = ct_members.get_mut(member) {
                    merge_associated_requirement(existing, requirement, member)?;
                } else {
                    ct_members.insert(member.clone(), requirement.clone());
                }
            }
            for (member, constraint) in &inherited.comptime_constraints {
                ct_constraints
                    .entry(member.clone())
                    .or_insert_with(|| constraint.clone());
            }
        }
        for member in comptime_members {
            let requirement = self.ct_member_req_from_anno(&member.params, &member.ty)?;
            if !member.where_clauses.is_empty() {
                let constraints = member
                    .where_clauses
                    .iter()
                    .map(|condition| self.compile_where_clause(condition))
                    .collect::<Result<Vec<_>, _>>()?;
                if let Some(previous) =
                    ct_constraints.insert(member.name.clone(), constraints.clone())
                {
                    // Implication compares whole declarations, so fold each
                    // clause list to one conjunction; the stored list keeps
                    // per-clause messages.
                    let previous = fold_constraint_conjunction(&previous);
                    let declared = fold_constraint_conjunction(&constraints);
                    if !generic_constraint_implies(&previous, &declared)
                        && !generic_constraint_implies(&declared, &previous)
                    {
                        return Err(TypeError::Unsupported(format!(
                            "conflicting inherited constraints on associated member '{}'",
                            member.name
                        )));
                    }
                }
            }
            if let Some(existing) = ct_members.get_mut(&member.name) {
                merge_associated_requirement(existing, &requirement, &member.name)?;
            } else {
                ct_members.insert(member.name.clone(), requirement);
            }
        }
        // Requirement signatures resolve `Self` to the abstract `Ty::SelfType`.
        let saved_self_ty = self.self_ty.replace(Ty::SelfType);
        let saved_self_decls = std::mem::take(&mut self.self_decls);
        self.trait_self_comptime.push(ct_members.clone());
        let result = (|| {
            let mut sigs: HashMap<String, Vec<MethodSig>> = HashMap::new();
            for parent in refines {
                let inherited = &self.traits[parent].methods;
                for (method, parent_sigs) in inherited {
                    let overloads = sigs.entry(method.clone()).or_default();
                    for sig in parent_sigs {
                        if !overloads.contains(sig) {
                            overloads.push(sig.clone());
                        }
                    }
                }
            }
            for m in methods {
                self.validate_origin_signature(&m.type_params, &m.params, m.self_origin.as_ref())?;
                if ct_members.contains_key(&m.name) {
                    return Err(TypeError::Redeclaration(m.name.clone()));
                }
                if let Some(feature) = Self::advanced_param_feature(
                    &m.params,
                    m.positional_only,
                    m.keyword_only,
                    true,
                    true,
                    false,
                ) {
                    return Err(TypeError::Unsupported(feature.to_string()));
                }
                if m.positional_only.is_some() || m.keyword_only.is_some() {
                    return Err(TypeError::Unsupported(
                        "positional-only/keyword-only markers on trait methods".to_string(),
                    ));
                }
                let mut decls = self.classify_params(&m.type_params)?;
                for condition in &m.where_clauses {
                    let constraint = self.compile_where_clause(condition)?;
                    let Some(last) = decls.last_mut() else {
                        return Err(TypeError::Unsupported(
                            "a where clause requires compile-time parameters".to_string(),
                        ));
                    };
                    match last {
                        ParamDecl::Type { constraints, .. }
                        | ParamDecl::Value { constraints, .. } => constraints.push(constraint),
                    }
                }
                self.tparams.push(type_scope(&decls));
                let signature = (|| {
                    Ok::<_, TypeError>((
                        self.param_tys(&m.params)?,
                        match &m.ret {
                            Some(SourceType::Ref { referent, .. }) => {
                                self.ty_from_anno(referent)?
                            }
                            Some(t) => self.ty_from_anno(t)?,
                            None => Ty::None,
                        },
                        self.declared_error(m.raises, m.raises_type.as_ref())?,
                    ))
                })();
                self.tparams.pop();
                let (all_types, ret, error) = signature?;
                let kw_variadic_idx = m
                    .params
                    .iter()
                    .position(|param| param.kind == mojito_ast::ast::ParamKind::KwVariadic);
                if let Some(index) = kw_variadic_idx {
                    self.kwargs_collector_ty(
                        all_types[index].clone(),
                        &format!("trait method '{}.{}' keyword collector", name, m.name),
                    )?;
                }
                let regular: Vec<_> = m
                    .params
                    .iter()
                    .enumerate()
                    .filter(|(_, param)| param.kind == mojito_ast::ast::ParamKind::Regular)
                    .collect();
                let regular_params: Vec<_> = regular.iter().map(|(_, param)| *param).collect();
                let ref_return = match &m.ret {
                    Some(SourceType::Ref { origin, .. }) => Some(lower_ref_sig(
                        origin.as_ref().ok_or_else(|| {
                            TypeError::Unsupported(
                                "reference return requires an origin".to_string(),
                            )
                        })?,
                        &m.type_params,
                        &regular_params,
                        0,
                    )?),
                    _ => None,
                };
                let sig = MethodSig {
                    decls,
                    availability: Vec::new(),
                    has_self: true,
                    params: regular
                        .iter()
                        .map(|(index, _)| all_types[*index].clone())
                        .collect(),
                    names: regular
                        .iter()
                        .map(|(_, param)| param.name.clone())
                        .collect(),
                    required: vec![true; regular.len()],
                    variadic: None,
                    variadic_index: None,
                    kw_variadic: kw_variadic_idx.map(|index| Box::new(all_types[index].clone())),
                    kw_variadic_index: kw_variadic_idx,
                    positional_only: m.positional_only,
                    keyword_only: m.keyword_only,
                    conventions: regular.iter().map(|(_, param)| param.convention).collect(),
                    ret,
                    raises: error.as_ref().is_some_and(|ty| *ty != Ty::Never),
                    error: error.map(Box::new),
                    self_convention: m.self_convention,
                    ref_params: lower_ref_param_sigs(&m.type_params, &regular_params, 0)?,
                    ref_return,
                    implicit: false,
                    parametric_origin_writes: Vec::new(),
                };
                let overloads = sigs.entry(m.name.clone()).or_default();
                if overloads.iter().any(|existing| {
                    same_method_shape(existing, &sig)
                        && (m.name != "__iter__" || existing.self_convention == sig.self_convention)
                }) {
                    return Err(TypeError::Redeclaration(m.name.clone()));
                }
                overloads.push(sig);
            }
            Ok(sigs)
        })();
        self.trait_self_comptime.pop();
        self.self_ty = saved_self_ty;
        self.self_decls = saved_self_decls;
        let methods = result?;
        self.traits.insert(
            name.to_string(),
            TraitInfo {
                refines: refines.to_vec(),
                methods,
                comptime_members: ct_members,
                comptime_constraints: ct_constraints,
            },
        );
        Ok(())
    }

    /// Register a struct and check its method bodies. A generic struct's type
    /// parameters are validated and kept in scope (as `Self.T`) for its fields
    /// and methods; field/method types referring to them become `Ty::Param`.
    /// Declared trait conformances are verified once the members are known.
    /// Check a struct declaration completely: shell, member types, method
    /// signatures, then conformance and bodies. `check_program` runs the first
    /// three phases for every top-level struct before the source-order walk,
    /// so this whole-declaration entry serves declarations the walk discovers
    /// without a predeclared shell.
    pub(super) fn check_struct(
        &mut self,
        declaration: &StructDeclaration<'_>,
    ) -> Result<(), TypeError> {
        self.check_struct_shell(declaration)?;
        self.check_struct_types(declaration)?;
        self.check_struct_method_signatures(declaration)?;
        self.check_struct_completion(declaration)
    }

    /// Register the struct's shell — classified parameters, generated-Tuple
    /// fixed arguments, and declaration-syntax facts — without resolving any
    /// member type. Shells for every top-level struct land before field,
    /// signature, or body resolution, so same-module struct declarations may
    /// reference each other regardless of order.
    pub(super) fn check_struct_shell(
        &mut self,
        declaration: &StructDeclaration<'_>,
    ) -> Result<(), TypeError> {
        let name = declaration.name;
        let type_params = declaration.type_params;
        if self.structs.contains_key(name) || self.traits.contains_key(name) {
            return Err(TypeError::Redeclaration(name.to_string()));
        }
        let decls = self.classify_params(type_params)?;
        self.generic_parameters.borrow_mut().insert(
            mojito_checked::checked::GenericSite::Struct {
                module: declaration.module.clone(),
                declaration: name.to_string(),
            },
            decls.clone(),
        );
        // A variadic struct template is compiled by compile-time specialization
        // (each instantiation is a concrete struct); the unspecialized template
        // has pack-dependent members and cannot be checked erased. The
        // elaborator's template shell is the one symbolic registration: a
        // retained generic body applies the template over its own parameters.
        if !declaration.template_shell
            && decls.iter().any(|decl| {
                matches!(
                    decl,
                    ParamDecl::Type { variadic: true, .. }
                        | ParamDecl::Value { variadic: true, .. }
                )
            })
        {
            return Err(TypeError::Unsupported(format!(
                "variadic struct '{name}' is compiled by compile-time specialization; instantiate it with explicit compile-time arguments (e.g. `{name}[Int, Bool](...)`) instead of checking the template"
            )));
        }

        // A generated public-Tuple implementation has erased its source pack
        // declaration, but its materialized `element_types` member retains the
        // concrete pack. Recover that checked identity before resolving `Self`
        // in fields and method signatures. The reserved specialization symbol
        // verifies provenance without decoding a mangled name.
        let generated_tuple = name.starts_with("Tuple$") || name.contains("$Tuple$");
        let saved_forward_types = std::mem::replace(
            &mut self.allow_generated_tuple_forward_types,
            generated_tuple,
        );
        let saved_type_params =
            std::mem::replace(&mut self.enclosing_type_params, type_params.to_vec());
        let fixed_arguments = self.generated_tuple_arguments(name, declaration.associated);
        self.enclosing_type_params = saved_type_params;
        self.allow_generated_tuple_forward_types = saved_forward_types;
        let fixed_arguments = fixed_arguments?;

        let explicit_destroy_message = declaration
            .decorators
            .iter()
            .find(|decorator| decorator.path.len() == 1 && decorator.path[0] == "explicit_destroy")
            .map(|decorator| {
                if !decorator.kwargs.is_empty() || decorator.args.len() != 1 {
                    return Err(TypeError::Unsupported(
                        "@explicit_destroy requires exactly one positional string message"
                            .to_string(),
                    ));
                }
                match decorator.args.first().map(|arg| &arg.kind) {
                    Some(ExprKind::Str(message)) => Ok(message.clone()),
                    Some(_) => Err(TypeError::Unsupported(
                        "@explicit_destroy message must be a string literal".to_string(),
                    )),
                    None => unreachable!("decorator arity was checked above"),
                }
            })
            .transpose()?;
        let explicit_destructors = declaration
            .methods
            .iter()
            .filter(|method| {
                method.name != "__deinit__" && method.self_convention == Some(ArgConvention::Deinit)
            })
            .map(|method| (method.name.clone(), method.raises))
            .collect::<HashMap<_, _>>();
        self.structs.insert(
            name.to_string(),
            StructInfo {
                decls,
                source_params: type_params.to_vec(),
                fixed_arguments,
                conforms: declaration.conforms.to_vec(),
                callable_conformance: None,
                callable_target: None,
                conformance_conditions: declaration
                    .conformance_conditions
                    .iter()
                    .cloned()
                    .collect(),
                fields: Vec::new(),
                field_origin_arguments: HashMap::new(),
                associated: HashMap::new(),
                associated_constraints: HashMap::new(),
                parameterized_associated: HashMap::new(),
                methods: HashMap::new(),
                fieldwise_init: declaration.fieldwise_init,
                explicit_destroy_message,
                explicit_destructors,
            },
        );
        Ok(())
    }

    /// Enter the member-resolution scope of a shelled struct: its parameters
    /// are in scope as `Self.T` / `Self.n` (type parameters as `Ty::Param`,
    /// value parameters as symbolic `CtValue::Param`) and bare `Self` is the
    /// struct type. Returns the `Self` type and the saved outer scope for
    /// `exit_struct_scope`.
    fn enter_struct_scope(
        &mut self,
        declaration: &StructDeclaration<'_>,
    ) -> Result<(Ty, SavedStructScope), TypeError> {
        let name = declaration.name;
        let info = self.structs.get(name).ok_or_else(|| {
            TypeError::InvariantViolation(format!(
                "struct '{name}' was not registered before member resolution"
            ))
        })?;
        let decls = info.decls.clone();
        let fixed_arguments = info.fixed_arguments.clone();
        let generated_tuple = name.starts_with("Tuple$") || name.contains("$Tuple$");
        let self_ty = Ty::Struct(
            name.to_string(),
            fixed_arguments.unwrap_or_else(|| decls.iter().map(param_as_arg).collect()),
        );
        let saved = SavedStructScope {
            forward_types: std::mem::replace(
                &mut self.allow_generated_tuple_forward_types,
                generated_tuple,
            ),
            type_params: std::mem::replace(
                &mut self.enclosing_type_params,
                declaration.type_params.to_vec(),
            ),
            self_decls: std::mem::replace(&mut self.self_decls, decls),
            self_ty: self.self_ty.replace(self_ty.clone()),
            bundled_stdlib: std::mem::replace(
                &mut self.bundled_stdlib_declaration,
                super::is_bundled_stdlib_source(super::bundled_struct_source(
                    declaration.module.as_deref(),
                    declaration.methods,
                )),
            ),
        };
        Ok((self_ty, saved))
    }

    fn exit_struct_scope(&mut self, saved: SavedStructScope) {
        self.self_decls = saved.self_decls;
        self.enclosing_type_params = saved.type_params;
        self.self_ty = saved.self_ty;
        self.allow_generated_tuple_forward_types = saved.forward_types;
        self.bundled_stdlib_declaration = saved.bundled_stdlib;
    }

    /// Resolve field, associated-member, and callable-conformance types into
    /// the registered shell.
    pub(super) fn check_struct_types(
        &mut self,
        declaration: &StructDeclaration<'_>,
    ) -> Result<(), TypeError> {
        let name = declaration.name;
        if !declaration.where_clauses.is_empty() {
            let has_constraint_binder = self
                .structs
                .get(name)
                .ok_or_else(|| {
                    TypeError::InvariantViolation(format!(
                        "struct '{name}' was not registered before applying its where clauses"
                    ))
                })?
                .decls
                .last()
                .is_some();
            for condition in declaration.where_clauses {
                let constraint = self.compile_where_clause(condition)?;
                if !has_constraint_binder && declaration.type_params.is_empty() {
                    self.validate_declaration_constraint(name, &constraint)?;
                }
                let info = self
                    .structs
                    .get_mut(name)
                    .expect("struct existence checked above");
                if let Some(last) = info.decls.last_mut() {
                    match last {
                        ParamDecl::Type { constraints, .. }
                        | ParamDecl::Value { constraints, .. } => {
                            constraints.push(constraint);
                        }
                    }
                }
            }
            let updated_decls = self
                .structs
                .get(name)
                .expect("struct existence checked above")
                .decls
                .clone();
            self.generic_parameters.borrow_mut().insert(
                mojito_checked::checked::GenericSite::Struct {
                    module: declaration.module.clone(),
                    declaration: name.to_string(),
                },
                updated_decls,
            );
        }
        // A template shell has no members to resolve: its fields depend on the
        // pack, and it exists only to type symbolic applications.
        if declaration.template_shell {
            return Ok(());
        }
        let (_, saved) = self.enter_struct_scope(declaration)?;
        let resolved = self.resolve_struct_member_types(declaration);
        self.exit_struct_scope(saved);
        let (
            fields,
            field_origin_arguments,
            associated,
            associated_constraints,
            parameterized_associated,
            callable_conformance,
        ) = resolved?;
        if callable_conformance
            .as_ref()
            .is_some_and(|ty| !matches!(ty, Ty::Func { .. }))
        {
            return Err(TypeError::Unsupported(
                "callable conformance must be a def(...) function type".to_string(),
            ));
        }
        let info = self
            .structs
            .get_mut(name)
            .expect("struct shell is registered before member-type resolution");
        info.fields = fields;
        info.field_origin_arguments = field_origin_arguments;
        info.associated = associated;
        info.associated_constraints = associated_constraints;
        info.parameterized_associated = parameterized_associated;
        info.callable_conformance = callable_conformance;
        Ok(())
    }

    fn resolve_struct_member_types(
        &mut self,
        declaration: &StructDeclaration<'_>,
    ) -> Result<StructMemberTypes, TypeError> {
        let name = declaration.name;
        // Associated members resolve BEFORE fields and are installed into the
        // registered shell right away: a field's type may apply the struct's
        // own comptime alias (`var iter: Self.dict_entry_iter`). Member bodies
        // reference only `Self` parameters and registered struct shells, never
        // fields, so this ordering is well-founded.
        let (associated_values, associated_constraints, parameterized_associated) =
            self.check_struct_associated(declaration.associated)?;
        if let Some(info) = self.structs.get_mut(name) {
            info.associated = associated_values.clone();
            info.associated_constraints = associated_constraints.clone();
            info.parameterized_associated = parameterized_associated.clone();
        }
        // Field types resolve with every module struct shell registered, so a
        // field may reference a struct declared later in the module (by-value
        // self-containment is rejected separately at completion); duplicate
        // field names are a redeclaration.
        let mut field_tys: Vec<(String, Ty)> = Vec::new();
        let mut field_origin_arguments = HashMap::new();
        let self_display = self.self_ty.as_ref().map(|ty| ty.to_string());
        for (field_index, f) in declaration.fields.iter().enumerate() {
            if field_tys.iter().any(|(n, _)| n == &f.name) {
                return Err(TypeError::Redeclaration(f.name.clone()));
            }
            // A field's `Self.name` resolves declared parameters and (now
            // pre-installed) associated members; a residual miss on `Self`
            // itself still reports an unknown parameter. Field annotations are
            // storage positions: explicit origin slots must be bound — except
            // in compiler-generated specializations (`$`-mangled names),
            // whose annotations reconstruct already-checked types with the
            // origin identity legitimately erased.
            let ty = if declaration.name.contains('$') {
                self.ty_from_anno(&f.ty)
            } else {
                self.resolve_storage_annotation(&f.ty, super::StorageStrictness::Full)
            }
            .map_err(|error| match error {
                TypeError::NoSuchAssociatedType {
                    object_type,
                    member,
                } if Some(&object_type) == self_display.as_ref() => {
                    TypeError::UnknownSelfParam(member)
                }
                other => other,
            })?;
            if Self::type_contains_unsafe_any_pointer(&ty) {
                return Err(TypeError::Unsupported(format!(
                    "field '{}' cannot hide a MutUnsafeAnyOrigin or ImmUnsafeAnyOrigin pointer",
                    f.name
                )));
            }
            super::type_resolution::reject_stored_callable_type(
                &ty,
                &format!("the type of struct field '{}'", f.name),
            )?;
            self.declaration_types.borrow_mut().insert(
                mojito_checked::checked::AnnotationSite::StructField {
                    module: declaration.module.clone(),
                    declaration: name.to_string(),
                    field: field_index,
                },
                ty.clone(),
            );
            if let Some(bindings) =
                self.field_origin_binder_arguments(&f.ty, declaration.associated)
            {
                field_origin_arguments.insert(f.name.clone(), bindings);
            }
            field_tys.push((f.name.clone(), ty));
        }
        let callable_conformance = declaration
            .callable_conformance
            .as_ref()
            .map(|annotation| self.ty_from_anno(annotation))
            .transpose()?;
        Ok((
            field_tys,
            field_origin_arguments,
            associated_values,
            associated_constraints,
            parameterized_associated,
            callable_conformance,
        ))
    }

    /// Lower and register every method signature of a shelled struct.
    pub(super) fn check_struct_method_signatures(
        &mut self,
        declaration: &StructDeclaration<'_>,
    ) -> Result<(), TypeError> {
        let name = declaration.name;
        let (_, saved) = self.enter_struct_scope(declaration)?;
        let result = self.register_struct_method_signatures(declaration);
        self.exit_struct_scope(saved);
        result?;
        // `@fieldwise_init` and a hand-written `__init__` both define a
        // constructor; having both is a conflict (the decorator *generates*
        // `__init__`).
        if declaration.fieldwise_init
            && self
                .structs
                .get(name)
                .is_some_and(|i| i.methods.contains_key("__init__"))
        {
            return Err(TypeError::ConflictingConstructor(name.to_string()));
        }
        Ok(())
    }

    fn register_struct_method_signatures(
        &mut self,
        declaration: &StructDeclaration<'_>,
    ) -> Result<(), TypeError> {
        let name = declaration.name;
        for (method_index, m) in declaration.methods.iter().enumerate() {
            let method_name = lifecycle_method_name(m);
            // A template shell registers the signatures that resolve
            // symbolically and skips the rest (pack-dependent shapes): the
            // concrete specialization checks every method.
            let method_decls = match self.classify_params(&m.type_params) {
                Ok(decls) => decls,
                Err(_) if declaration.template_shell => continue,
                Err(error) => return Err(error),
            };
            self.generic_parameters.borrow_mut().insert(
                mojito_checked::checked::GenericSite::Method {
                    module: declaration.module.clone(),
                    declaration: name.to_string(),
                    method: method_index,
                },
                method_decls.clone(),
            );
            self.tparams.push(type_scope(&method_decls));
            let saved_method_type_params = self.enclosing_type_params.clone();
            let saved_struct_count = self
                .enclosing_struct_type_params
                .replace(saved_method_type_params.len());
            self.enclosing_type_params.extend(m.type_params.clone());
            // A per-instantiation clone declares its receiver type: its
            // signature resolves `Self` against that instance.
            let receiver_override = m
                .self_ty
                .as_ref()
                .map(|ty| self.ty_from_anno(ty))
                .transpose()?;
            let saved_self_ty = receiver_override
                .as_ref()
                .map(|ty| self.self_ty.replace(ty.clone()));
            let signature = (|| {
                let all_types = self.param_tys(&m.params)?;
                let sig = self.method_sig(m, method_decls, &all_types)?;
                Ok::<_, TypeError>((all_types, sig))
            })();
            if let Some(saved) = saved_self_ty {
                self.self_ty = saved;
            }
            if let Some(receiver) = receiver_override {
                self.declaration_types.borrow_mut().insert(
                    mojito_checked::checked::AnnotationSite::MethodSelf {
                        module: declaration.module.clone(),
                        declaration: name.to_string(),
                        method: method_index,
                    },
                    receiver,
                );
            }
            self.enclosing_type_params = saved_method_type_params;
            self.enclosing_struct_type_params.set(saved_struct_count);
            self.tparams.pop();
            let (all_types, mut sig) = match signature {
                Ok(signature) => signature,
                Err(_) if declaration.template_shell => continue,
                Err(error) => return Err(error),
            };
            if let Some(info) = self.structs.get(name) {
                sig.parametric_origin_writes =
                    parametric_origin_writes_in_body(&m.body, &info.fields);
            }
            for (param, ty) in all_types.iter().enumerate() {
                self.declaration_types.borrow_mut().insert(
                    mojito_checked::checked::AnnotationSite::MethodParam {
                        module: declaration.module.clone(),
                        declaration: name.to_string(),
                        method: method_index,
                        param,
                    },
                    ty.clone(),
                );
            }
            self.declaration_types.borrow_mut().insert(
                mojito_checked::checked::AnnotationSite::MethodReturn {
                    module: declaration.module.clone(),
                    declaration: name.to_string(),
                    method: method_index,
                },
                sig.ret.clone(),
            );
            self.declaration_effects.borrow_mut().insert(
                mojito_checked::checked::AnnotationSite::MethodReturn {
                    module: declaration.module.clone(),
                    declaration: name.to_string(),
                    method: method_index,
                },
                mojito_checked::checked::DeclarationEffect {
                    raises: sig.raises,
                    error: sig.raises.then(|| sig.error.as_deref().cloned()).flatten(),
                    returns_reference: sig.ref_return.is_some(),
                },
            );
            let info = self.structs.get_mut(name).ok_or_else(|| {
                TypeError::InvariantViolation(format!("struct '{name}' was not registered"))
            })?;
            let overloads = info.methods.entry(method_name.to_string()).or_default();
            if overloads.iter().any(|existing| {
                same_method_shape(existing, &sig)
                    && (!mojito_symbol::symbol::receiver_overloaded_method(method_name)
                        || existing.self_convention == sig.self_convention)
            }) {
                if declaration.template_shell {
                    continue;
                }
                return Err(TypeError::Redeclaration(method_name.to_string()));
            }
            if method_name == "__setitem__"
                && overloads
                    .iter()
                    .any(|existing| competing_setitem_value_shapes(existing, &sig))
            {
                return Err(TypeError::Unsupported(
                    "competing '__setitem__' overloads for the same indices: one takes the \
                     assignment value positionally and the other as keyword-only '*, value:'; \
                     declare a single setter shape"
                        .to_string(),
                ));
            }
            overloads.push(sig);
        }
        Ok(())
    }

    /// Verify declared conformances, select the callable target, and check
    /// method bodies — the source-order completion phase of a struct whose
    /// shell, member types, and method signatures are already registered.
    pub(super) fn check_struct_completion(
        &mut self,
        declaration: &StructDeclaration<'_>,
    ) -> Result<(), TypeError> {
        for tr in declaration.conforms {
            self.check_trait_name(tr)?;
        }
        // A template shell has no bodies and verifies no conformance: the
        // concrete specialization does both.
        if declaration.template_shell {
            return Ok(());
        }
        self.reject_value_field_self_containment(declaration.name)?;
        let (self_ty, saved) = self.enter_struct_scope(declaration)?;
        let result = self.verify_conformance_and_bodies(declaration, &self_ty);
        self.exit_struct_scope(saved);
        result
    }

    fn verify_conformance_and_bodies(
        &mut self,
        declaration: &StructDeclaration<'_>,
        self_ty: &Ty,
    ) -> Result<(), TypeError> {
        let name = declaration.name;
        let methods = declaration.methods;
        // Verify each declared conformance now that the method signatures exist.
        for tr in declaration.conforms {
            self.verify_conformance(name, tr, self_ty)?;
        }
        if let Some(expected) = self
            .structs
            .get(name)
            .and_then(|info| info.callable_conformance.clone())
        {
            let Some(call_methods) = self
                .structs
                .get(name)
                .and_then(|info| info.methods.get("__call__"))
            else {
                return Err(TypeError::MissingTraitMethod {
                    struct_name: name.to_string(),
                    trait_name: expected.to_string(),
                    method: "__call__".to_string(),
                });
            };
            let matching = call_methods
                .iter()
                .filter(|method| {
                    let actual = method_callable_ty(method);
                    coerces(&actual, &expected) && coerces(&expected, &actual)
                })
                .collect::<Vec<_>>();
            let [selected] = matching.as_slice() else {
                return Err(TypeError::TraitMethodMismatch {
                    struct_name: name.to_string(),
                    trait_name: expected.to_string(),
                    method: "__call__".to_string(),
                });
            };
            let target = if call_methods.len() == 1 {
                format!("{name}.__call__")
            } else {
                method_lowered_name(
                    name,
                    "__call__",
                    selected,
                    self.self_instance_ty(name).as_ref(),
                )
            };
            self.structs
                .get_mut(name)
                .expect("callable struct remains registered")
                .callable_target = Some(target);
        }
        // Method bodies, each with `self` bound to this struct at its own type
        // parameters (so `self.field : Ty::Param` inside a generic struct).
        let mut overload_indices = HashMap::<String, usize>::new();
        for (method_index, m) in methods.iter().enumerate() {
            let method_name = lifecycle_method_name(m).to_string();
            let overload_index = *overload_indices.entry(method_name.clone()).or_default();
            *overload_indices
                .get_mut(&method_name)
                .expect("inserted above") += 1;
            // A per-instantiation clone checks with `self`/`Self` bound to its
            // declared receiver instance rather than the parametric struct.
            let receiver_override = m
                .self_ty
                .as_ref()
                .map(|ty| self.ty_from_anno(ty))
                .transpose()?;
            let method_self_ty = receiver_override.clone().unwrap_or_else(|| self_ty.clone());
            let method_self_ty_override = receiver_override.clone();
            let saved_self_ty = receiver_override.map(|ty| self.self_ty.replace(ty));
            let result = self.check_method(
                &method_self_ty,
                m,
                declaration.module.clone(),
                name,
                method_index,
                overload_index,
            );
            if let Some(saved) = saved_self_ty {
                self.self_ty = saved;
            }
            // A clone's failure is a post-instantiation error: report the
            // instance and the source method the clone was minted from.
            if let (Err(error), Some(receiver)) = (&result, &method_self_ty_override) {
                return Err(TypeError::PostInstantiation {
                    receiver: receiver.to_string(),
                    method: m.name.split('$').next().unwrap_or(&m.name).to_string(),
                    error: Box::new(error.clone()),
                });
            }
            result?;
        }
        Ok(())
    }

    /// With every module struct shell registered before field resolution, a
    /// struct may reference itself — or a mutual peer — in a field type, but
    /// only behind reference or pointer indirection: a by-value field cycle
    /// has no finite layout. Runs at completion, when every module struct's
    /// fields are resolved.
    fn reject_value_field_self_containment(&self, name: &str) -> Result<(), TypeError> {
        fn visit_struct(
            checker: &Checker,
            root: &str,
            current: &Ty,
            visiting: &mut HashSet<String>,
        ) -> Result<(), TypeError> {
            let Ty::Struct(current_name, args) = current else {
                return Ok(());
            };
            if current_name == root {
                return Err(TypeError::Unsupported(format!(
                    "struct '{root}' cannot contain itself by value; use reference or pointer indirection"
                )));
            }
            if !visiting.insert(current_name.clone()) {
                return Ok(());
            }
            let Some(info) = checker.structs.get(current_name) else {
                return Ok(());
            };
            let subst = struct_subst(&info.decls, args);
            for (_, field_ty) in info.fields.clone() {
                visit_field(checker, root, &substitute(&field_ty, &subst), visiting)?;
            }
            Ok(())
        }
        fn visit_field(
            checker: &Checker,
            root: &str,
            ty: &Ty,
            visiting: &mut HashSet<String>,
        ) -> Result<(), TypeError> {
            match ty {
                // A handle or pointer breaks the by-value containment chain.
                Ty::Ref(_) | Ty::Pointer { .. } => Ok(()),
                _ => visit_struct(checker, root, ty, visiting),
            }
        }
        let Some(info) = self.structs.get(name) else {
            return Ok(());
        };
        let mut visiting = HashSet::new();
        for (_, field_ty) in info.fields.clone() {
            visit_field(self, name, &field_ty, &mut visiting)?;
        }
        Ok(())
    }

    /// Verify that struct `name` (whose `Self` type is `self_ty`) implements
    /// every method required by trait `tr`, with a matching signature. A few
    /// built-in marker traits have real lifecycle semantics; other built-ins
    /// remain shallow recognized bounds until their corresponding feature grows.
    pub(super) fn verify_conformance(
        &self,
        name: &str,
        tr: &str,
        self_ty: &Ty,
    ) -> Result<(), TypeError> {
        if let Some(condition) = self
            .structs
            .get(name)
            .and_then(|info| info.conformance_conditions.get(tr))
        {
            // Validate the declaration shape even for builtin marker traits.
            // Truth is evaluated at each concrete use, but a malformed
            // `(condition, message)` tuple is always a declaration error.
            self.compile_where_clause(condition)?;
        }
        // The focused checker can recognize protocol bounds without linking the
        // implicit prelude, but a registered nominal trait is authoritative.
        // In production `Iterator`/`Iterable` are ordinary stdlib traits; the
        // builtin compatibility spelling must not bypass their requirements.
        if BUILTIN_TRAITS.contains(&tr) && !self.traits.contains_key(tr) {
            return self.verify_builtin_conformance(name, tr, self_ty);
        }
        let trait_info = match self.traits.get(tr) {
            Some(info) => info,
            None => return Ok(()),
        };
        let struct_info = self.structs.get(name).ok_or_else(|| {
            TypeError::InvariantViolation(format!(
                "struct '{name}' was not registered before conformance checking"
            ))
        })?;
        let conformance_assumption = struct_info
            .conformance_conditions
            .get(tr)
            .map(|condition| self.compile_where_clause(condition))
            .transpose()?;
        for (mname, req_sigs) in &trait_info.methods {
            let got_sigs =
                struct_info
                    .methods
                    .get(mname)
                    .ok_or_else(|| TypeError::MissingTraitMethod {
                        struct_name: name.to_string(),
                        trait_name: tr.to_string(),
                        method: mname.clone(),
                    })?;
            // The requirement's `Self` becomes this struct's type. Receiver
            // conventions are part of the trait method contract.
            for req_sig in req_sigs {
                let want =
                    MethodSig {
                        decls: req_sig.decls.clone(),
                        availability: req_sig.availability.clone(),
                        has_self: true,
                        params: req_sig
                            .params
                            .iter()
                            .map(|t| self.resolve_assoc_ty(&substitute_self(t, self_ty)))
                            .collect(),
                        names: req_sig.names.clone(),
                        required: req_sig.required.clone(),
                        variadic: req_sig.variadic.as_ref().map(|ty| {
                            Box::new(self.resolve_assoc_ty(&substitute_self(ty, self_ty)))
                        }),
                        variadic_index: req_sig.variadic_index,
                        kw_variadic: req_sig.kw_variadic.as_ref().map(|ty| {
                            Box::new(self.resolve_assoc_ty(&substitute_self(ty, self_ty)))
                        }),
                        kw_variadic_index: req_sig.kw_variadic_index,
                        positional_only: req_sig.positional_only,
                        keyword_only: req_sig.keyword_only,
                        conventions: req_sig.conventions.clone(),
                        ret: self.resolve_assoc_ty(&substitute_self(&req_sig.ret, self_ty)),
                        raises: req_sig.raises,
                        error: req_sig.error.as_ref().map(|error| {
                            Box::new(self.resolve_assoc_ty(&substitute_self(error, self_ty)))
                        }),
                        self_convention: req_sig.self_convention,
                        ref_params: req_sig.ref_params.clone(),
                        ref_return: req_sig.ref_return.clone(),
                        implicit: req_sig.implicit,
                        parametric_origin_writes: req_sig.parametric_origin_writes.clone(),
                    };
                if !got_sigs.iter().any(|got| {
                    self.method_satisfies_requirement_under(
                        got,
                        &want,
                        conformance_assumption.as_ref(),
                        mname == "__next__",
                    )
                }) {
                    return Err(TypeError::TraitMethodMismatch {
                        struct_name: name.to_string(),
                        trait_name: tr.to_string(),
                        method: mname.clone(),
                    });
                }
            }
        }
        for (member, req) in &trait_info.comptime_members {
            let witness_constraints = struct_info
                .parameterized_associated
                .get(member)
                .map(|definition| definition.availability.as_slice())
                .filter(|constraints| !constraints.is_empty())
                .or_else(|| {
                    struct_info
                        .associated_constraints
                        .get(member)
                        .map(Vec::as_slice)
                })
                .unwrap_or(&[]);
            let requirement_premise = trait_info
                .comptime_constraints
                .get(member)
                .map(|premises| fold_constraint_conjunction(premises));
            for constraint in witness_constraints {
                if matches!(constraint, GenericConstraint::Bool(true)) {
                    continue;
                }
                let covered = conformance_assumption
                    .as_ref()
                    .is_some_and(|premise| generic_constraint_implies(premise, constraint))
                    || requirement_premise
                        .as_ref()
                        .is_some_and(|premise| generic_constraint_implies(premise, constraint));
                if !covered {
                    let reason = match constraint {
                        GenericConstraint::WithMessage(_, message) => {
                            format!("constraint failed: {message}")
                        }
                        _ => {
                            format!("associated member constraint is not satisfied: {constraint:?}")
                        }
                    };
                    return Err(TypeError::BadCall {
                        func: format!("{name}.{member}"),
                        reason,
                    });
                }
            }
            // A parameterized associated type is stored separately and cannot be
            // eagerly evaluated. Its parameterization was validated at declaration;
            // require the definition's explicit parameter count to match the
            // requirement's, and that the requirement is itself parameterized.
            if let Some(parameterized) = struct_info.parameterized_associated.get(member) {
                let CtMemberReq::Type { bounds, params } = req else {
                    return Err(TypeError::TraitComptimeMemberMismatch {
                        struct_name: name.to_string(),
                        trait_name: tr.to_string(),
                        member: member.clone(),
                    });
                };
                let required = params.iter().filter(|p| !p.infer_only).count();
                let provided = parameterized
                    .params
                    .iter()
                    .filter(|p| !p.infer_only)
                    .count();
                if required != provided {
                    return Err(TypeError::TraitComptimeMemberMismatch {
                        struct_name: name.to_string(),
                        trait_name: tr.to_string(),
                        member: member.clone(),
                    });
                }
                // Enforce the requirement's bound, not just its arity:
                // instantiate the definition's template with placeholder
                // explicit arguments and check the result. An explicit value
                // parameter has no fabricable placeholder witness; that shape
                // keeps the arity-only contract.
                let placeholders = parameterized
                    .params
                    .iter()
                    .filter(|param| !param.infer_only)
                    .map(|param| match constraints::assoc_param_kind(param) {
                        constraints::AssocParamKind::Origin => {
                            Some(TyArg::Origin(mojito_types::origin::Origin::Untracked {
                                mutable: false,
                            }))
                        }
                        constraints::AssocParamKind::Type => Some(TyArg::Ty(Ty::Param {
                            name: param.name.clone(),
                            bounds: param.bounds.clone(),
                            callable_bound: None,
                        })),
                        constraints::AssocParamKind::Value => None,
                    })
                    .collect::<Option<Vec<_>>>();
                if let Some(placeholders) = placeholders {
                    let instantiated =
                        self.associated_type_from_base(self_ty, member, &placeholders)?;
                    let satisfied = bounds.iter().all(|bound| {
                        self.conforms_to_under_assumption(
                            &instantiated,
                            bound,
                            conformance_assumption.as_ref(),
                        )
                    });
                    if !satisfied {
                        return Err(TypeError::TraitComptimeMemberMismatch {
                            struct_name: name.to_string(),
                            trait_name: tr.to_string(),
                            member: member.clone(),
                        });
                    }
                }
                continue;
            }
            let got = struct_info.associated.get(member).ok_or_else(|| {
                TypeError::MissingTraitComptimeMember {
                    struct_name: name.to_string(),
                    trait_name: tr.to_string(),
                    member: member.clone(),
                }
            })?;
            if !self.ct_member_satisfies(got, req, self_ty, conformance_assumption.as_ref()) {
                return Err(TypeError::TraitComptimeMemberMismatch {
                    struct_name: name.to_string(),
                    trait_name: tr.to_string(),
                    member: member.clone(),
                });
            }
        }
        Ok(())
    }

    pub(super) fn method_satisfies_requirement_under(
        &self,
        got: &MethodSig,
        required: &MethodSig,
        conformance_assumption: Option<&GenericConstraint>,
        allow_copyable_iterator_reference: bool,
    ) -> bool {
        let availability_is_covered = got.availability.iter().all(|constraint| {
            required
                .availability
                .iter()
                .any(|premise| generic_constraint_implies(premise, constraint))
                || conformance_assumption
                    .is_some_and(|premise| generic_constraint_implies(premise, constraint))
        });
        if !availability_is_covered {
            return false;
        }
        // Availability was proved above. Normalize it to the requirement before
        // comparing the remainder of the callable contract.
        let mut normalized = got.clone();
        normalized.availability = required.availability.clone();
        if normalized.ref_return != required.ref_return {
            let copyable_reference_refinement = allow_copyable_iterator_reference
                && normalized.ref_return.is_some()
                && required.ref_return.is_none()
                && normalized.ret == required.ret
                && self.is_copyable_under_assumption(&required.ret, conformance_assumption);
            if !copyable_reference_refinement {
                return false;
            }
            // The concrete reference ABI is observed as the abstract value ABI
            // through an explicit checked result adapter. Normalize only after
            // proving that directional refinement is legal.
            normalized.ref_return = None;
        }
        method_satisfies_requirement(&normalized, required)
    }

    /// Prove Copyable without treating a conditional marker declaration as
    /// unconditional. This is used at a conformance boundary where the
    /// conformer's `where` premise is available even though its type remains
    /// symbolic.
    fn is_copyable_under_assumption(
        &self,
        ty: &Ty,
        assumption: Option<&GenericConstraint>,
    ) -> bool {
        match ty {
            Ty::ComptimeList(element) => self.is_copyable_under_assumption(element, assumption),
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => elements
                .iter()
                .all(|element| self.is_copyable_under_assumption(element, assumption)),
            Ty::Variant(alternatives) => alternatives
                .iter()
                .all(|alternative| self.is_copyable_under_assumption(alternative, assumption)),
            Ty::Param { name, bounds, .. } => {
                bounds.iter().any(|bound| {
                    matches!(bound.as_str(), "Copyable" | "ImplicitlyCopyable")
                        || self.trait_refines(bound, "Copyable")
                }) || ["Copyable", "ImplicitlyCopyable"].iter().any(|required| {
                    let needed = GenericConstraint::Conforms {
                        param: name.trim_start_matches('*').to_string(),
                        trait_name: (*required).to_string(),
                    };
                    assumption.is_some_and(|known| generic_constraint_implies(known, &needed))
                })
            }
            Ty::Assoc { .. } => self.assoc_member_bound_proves(ty, "Copyable"),
            Ty::Struct(name, arguments) => {
                let Some(info) = self.structs.get(name) else {
                    // A compatibility spelling may produce an opaque nominal
                    // type without registering its declaration. That is not a
                    // Copyable proof strong enough to change a method ABI.
                    return false;
                };
                let environment: HashMap<&str, &TyArg> = info
                    .decls
                    .iter()
                    .zip(arguments)
                    .map(|(declaration, argument)| {
                        (declaration.name().trim_start_matches('*'), argument)
                    })
                    .collect();
                if let Some(methods) = info.methods.get("__copyinit__") {
                    // A declared copy initializer suppresses the fieldwise copy
                    // path. Its method availability is therefore the real
                    // capability; do not fall through to a nominal marker and
                    // mistake an unavailable initializer for a usable copy.
                    return methods.iter().any(|method| {
                        method.availability.iter().all(|condition| {
                            self.eval_constraint_under_assumption(
                                condition,
                                &environment,
                                assumption,
                                &mut HashSet::new(),
                            )
                        })
                    });
                }
                info.conforms.iter().any(|declared| {
                    if !matches!(declared.as_str(), "Copyable" | "ImplicitlyCopyable") {
                        return false;
                    }
                    let Some(condition) = info.conformance_conditions.get(declared) else {
                        return true;
                    };
                    let Ok(condition) = self.compile_where_clause(condition) else {
                        return false;
                    };
                    self.eval_constraint_under_assumption(
                        &condition,
                        &environment,
                        assumption,
                        &mut HashSet::new(),
                    )
                })
            }
            Ty::VariadicPack(element) => self.is_copyable_under_assumption(element, assumption),
            // These types still depend on substitution or trait context. The
            // ordinary capability helper defaults unknown concrete spellings to
            // copyable for compatibility, but that is not a proof strong enough
            // to change an abstract method ABI.
            Ty::Dependent(_) | Ty::SelfType | Ty::Infer => false,
            _ => self.is_copyable(ty),
        }
    }

    pub(super) fn verify_builtin_conformance(
        &self,
        name: &str,
        tr: &str,
        self_ty: &Ty,
    ) -> Result<(), TypeError> {
        let ok = match tr {
            "Copyable" => self.struct_copyable_conformance_ok(name),
            "ImplicitlyCopyable" => self.struct_implicitly_copyable_conformance_ok(name),
            // A declared narrowing conformance (`Movable where False`) must
            // verify at declaration like `Deinitable where False`;
            // effectiveness is enforced at the transfer/consuming use sites.
            "Movable" => true,
            "Deinitable" => true,
            "Indexer" => self.structs.get(name).is_some_and(|info| {
                info.methods.get("__mlir_index__").is_some_and(|methods| {
                    methods.iter().any(|method| {
                        method.has_self && method.params.is_empty() && method.ret == Ty::Int
                    })
                })
            }),
            "Writer" => self.structs.get(name).is_some_and(|info| {
                info.methods.get("write_string").is_some_and(|methods| {
                    methods.iter().any(|method| {
                        // The payload parameter may be spelled as the
                        // compile-time literal or the nominal String; the VM
                        // write bridge materializes for the nominal spelling.
                        let payload = match method.params.as_slice() {
                            [Ty::StringLiteral] => true,
                            [Ty::Struct(name, args)] => {
                                args.is_empty() && mojito_symbol::symbol::is_stdlib_string_struct(name)
                            }
                            _ => false,
                        };
                        method.has_self
                            && method.self_convention == Some(ArgConvention::Mut)
                            && payload
                            && method.ret == Ty::None
                    })
                })
            }),
            "Hasher" => self.structs.get(name).is_some_and(|info| {
                let initializes = info.methods.get("__init__").is_some_and(|methods| {
                    methods.iter().any(|method| method.params.is_empty())
                });
                let updates = info.methods.get("update").is_some_and(|methods| {
                    methods.iter().any(|method| {
                        method.self_convention == Some(ArgConvention::Mut)
                            && method.params.len() == 1
                            && method.ret == Ty::None
                    })
                });
                let finishes = info.methods.get("finish").is_some_and(|methods| {
                    methods.iter().any(|method| {
                        method.params.is_empty()
                            && method.self_convention == Some(ArgConvention::Var)
                            && method.ret
                                == Ty::Simd {
                                    dtype: mojito_ast::ast::Dtype::UInt64,
                                    width: 1,
                                }
                    })
                });
                let simd_updates = info.methods.get("_update_with_simd").is_some_and(|methods| {
                    methods.iter().any(|method| {
                        method.self_convention == Some(ArgConvention::Mut)
                            && method.params
                                == [Ty::Simd {
                                    dtype: mojito_ast::ast::Dtype::UInt64,
                                    width: 1,
                                }]
                            && method.ret == Ty::None
                    })
                });
                let byte_updates = info.methods.get("_update_with_bytes").is_some_and(|methods| {
                    methods.iter().any(|method| {
                        method.self_convention == Some(ArgConvention::Mut)
                            && method.params.len() == 1
                            && method.ret == Ty::None
                    })
                });
                initializes && updates && simd_updates && byte_updates && finishes
            }),
            "Hashable" => self.structs.get(name).is_some_and(|info| {
                info.methods.get("__hash__").is_some_and(|methods| {
                    methods.iter().any(|method| {
                        method.has_self
                            && method.self_convention.is_none()
                            && method.params.len() == 1
                            && method.conventions == [Some(ArgConvention::Mut)]
                            && matches!(&method.params[0], Ty::Param { bounds, .. } if bounds.iter().any(|bound| bound == "Hasher"))
                            && method.ret == Ty::None
                    })
                })
            }),
            "Writable" => self.structs.get(name).is_some_and(|info| {
                ["write_to", "write_repr_to"].into_iter().all(|name| {
                    info.methods.get(name).is_none_or(|methods| {
                        methods.iter().any(|method| {
                            method.params.len() == 1
                                && method.conventions[0] == Some(ArgConvention::Mut)
                                && matches!(&method.params[0], Ty::Param { bounds, .. } if bounds.iter().any(|bound| bound == "Writer"))
                                && method.ret == Ty::None
                        })
                    })
                })
            }),
            // An operation trait with a known dunder signature requires the
            // struct to define that dunder (`Addable` needs `__add__`, etc.).
            // Layout/backend markers without a dunder remain accepted-but-shallow.
            _ => match builtin_trait_operation(tr) {
                Some(signature) => {
                    let dunder = signature.split('(').next().unwrap_or(signature);
                    self.structs.get(name).is_some_and(|info| {
                        info.methods
                            .get(dunder)
                            .is_some_and(|methods| methods.iter().any(|method| method.has_self))
                    })
                }
                None => true,
            },
        };
        if ok {
            Ok(())
        } else {
            Err(TypeError::TraitNotSatisfied {
                param: "Self".to_string(),
                ty: self_ty.to_string(),
                trait_name: tr.to_string(),
                reason: self.trait_failure_reason(self_ty, tr),
            })
        }
    }

    pub(super) fn ct_member_satisfies(
        &self,
        value: &CtValue,
        req: &CtMemberReq,
        self_ty: &Ty,
        conformance_assumption: Option<&GenericConstraint>,
    ) -> bool {
        match req {
            CtMemberReq::Value(expected) => self
                .ct_value_ty(value, self_ty)
                .is_some_and(|actual| coerces(&actual, expected)),
            CtMemberReq::Type { bounds, .. } => {
                let CtValue::Type(ty) = value else {
                    return false;
                };
                bounds.iter().all(|bound| {
                    self.conforms_to_under_assumption(ty, bound, conformance_assumption)
                })
            }
        }
    }

    pub(super) fn conforms_to_under_assumption(
        &self,
        ty: &Ty,
        required: &str,
        assumption: Option<&GenericConstraint>,
    ) -> bool {
        self.conforms_to_under_assumption_inner(ty, required, assumption, &mut HashSet::new())
    }

    pub(super) fn conforms_to_under_assumption_inner(
        &self,
        ty: &Ty,
        required: &str,
        assumption: Option<&GenericConstraint>,
        visiting: &mut HashSet<(String, String)>,
    ) -> bool {
        if self.conforms_to(ty, required) {
            return true;
        }
        if let Ty::Param { name, .. } = ty {
            let needed = GenericConstraint::Conforms {
                param: name.clone(),
                trait_name: required.to_string(),
            };
            return assumption.is_some_and(|known| generic_constraint_implies(known, &needed));
        }
        let Ty::Struct(name, args) = ty else {
            return false;
        };
        let key = (name.clone(), required.to_string());
        if !visiting.insert(key.clone()) {
            return false;
        }
        let result = self.structs.get(name).is_some_and(|info| {
            info.conforms.iter().any(|declared| {
                if declared != required && !self.trait_refines(declared, required) {
                    return false;
                }
                let Some(condition) = info.conformance_conditions.get(declared) else {
                    return true;
                };
                let Ok(condition) = self.compile_where_clause(condition) else {
                    return false;
                };
                let environment: HashMap<&str, &TyArg> = info
                    .decls
                    .iter()
                    .zip(args)
                    .map(|(decl, argument)| (decl.name().trim_start_matches('*'), argument))
                    .collect();
                self.eval_constraint_under_assumption(
                    &condition,
                    &environment,
                    assumption,
                    visiting,
                )
            })
        });
        visiting.remove(&key);
        result
    }

    pub(super) fn eval_constraint_under_assumption(
        &self,
        constraint: &GenericConstraint,
        environment: &HashMap<&str, &TyArg>,
        assumption: Option<&GenericConstraint>,
        visiting: &mut HashSet<(String, String)>,
    ) -> bool {
        use GenericConstraint::*;
        match constraint {
            WithMessage(condition, _) => self.eval_constraint_under_assumption(
                condition,
                environment,
                assumption,
                visiting,
            ),
            Conforms { param, trait_name } => environment
                .get(param.as_str())
                .is_some_and(|argument| match argument {
                    TyArg::Ty(ty) => self.conforms_to_under_assumption_inner(
                        ty,
                        trait_name,
                        assumption,
                        visiting,
                    ),
                    TyArg::Val(_) | TyArg::Origin(_) => false,
                }),
            ConformsPack { param, trait_name } => environment
                .get(param.as_str())
                .is_some_and(|argument| match argument {
                    TyArg::Val(CtValue::Tuple(values)) => values.iter().all(|value| {
                        matches!(value, CtValue::Type(ty) if self.conforms_to_under_assumption_inner(
                            ty,
                            trait_name,
                            assumption,
                            visiting,
                        ))
                    }),
                    _ => false,
                }),
            And(left, right) => {
                self.eval_constraint_under_assumption(left, environment, assumption, visiting)
                    && self.eval_constraint_under_assumption(
                        right,
                        environment,
                        assumption,
                        visiting,
                    )
            }
            Or(left, right) => {
                self.eval_constraint_under_assumption(left, environment, assumption, visiting)
                    || self.eval_constraint_under_assumption(
                        right,
                        environment,
                        assumption,
                        visiting,
                    )
            }
            // Do not derive a negative proposition from an unknown symbolic
            // fact. Exact non-conformance constraints continue through the
            // ordinary evaluator when their arguments are concrete.
            Not(_) => self.eval_generic_constraint(constraint, environment),
            _ => self.eval_generic_constraint(constraint, environment),
        }
    }

    pub(super) fn ct_value_ty(&self, value: &CtValue, self_ty: &Ty) -> Option<Ty> {
        match value {
            CtValue::Int(_) | CtValue::Param(_) => Some(Ty::Int),
            CtValue::UInt(_) => Some(Ty::UInt),
            CtValue::Float(_) => Some(Ty::Float64),
            CtValue::IntLiteral(_) => Some(Ty::IntLiteral),
            CtValue::FloatLiteral(_) => Some(Ty::FloatLiteral),
            CtValue::Bool(_) => Some(Ty::Bool),
            CtValue::Str(_) => Some(Ty::StringLiteral),
            CtValue::Dtype(_) => Some(Ty::Dtype),
            CtValue::Struct { name, .. } => Some(Ty::Struct(name.clone(), Vec::new())),
            CtValue::Tuple(values) => values
                .iter()
                .map(|v| self.ct_value_ty(v, self_ty))
                .collect::<Option<Vec<_>>>()
                .map(|elements| {
                    if matches!(self_ty, Ty::Tuple(_) | Ty::RuntimePack(_)) {
                        Ty::Tuple(elements)
                    } else {
                        nominal_tuple_type(elements)
                    }
                }),
            CtValue::List(values) => {
                let first = values.first()?;
                let elem = self.ct_value_ty(first, self_ty)?;
                if values.iter().skip(1).all(|v| {
                    self.ct_value_ty(v, self_ty)
                        .is_some_and(|ty| coerces(&ty, &elem))
                }) {
                    Some(list_type(elem))
                } else {
                    None
                }
            }
            CtValue::Type(_) | CtValue::Reflected(_) => {
                let _ = self_ty;
                None
            }
        }
    }

    /// Whether `ty` conforms to trait `tr`. Lifecycle marker built-ins are tied
    /// to observable ownership behavior; other built-ins remain recognized but
    /// shallow unless their feature has a dedicated checker path. A user trait is
    /// satisfied nominally: a struct must *declare* conformance, and a type
    /// parameter must carry `tr` among its bounds (so a bounded `T` can be
    /// forwarded to another `[U: tr]` parameter).
    pub(super) fn conforms_to(&self, ty: &Ty, tr: &str) -> bool {
        if self.has_assumed_conformance(ty, tr) {
            return true;
        }
        if let Ty::Param { bounds, .. } = ty
            && bounds.iter().any(|bound| bound == tr)
        {
            return true;
        }
        if BUILTIN_TRAITS.contains(&tr) && !self.traits.contains_key(tr) {
            return match tr {
                "AnyType" => true,
                "Copyable" => self.is_copyable(ty),
                "ImplicitlyCopyable" => self.is_implicitly_copyable(ty),
                "Movable" => self.is_movable(ty),
                "Deinitable" => self.is_deinitable(ty),
                "Hashable" => self.is_hashable(ty),
                "Writable" => {
                    // The discovery check runs before a `t"…"` occurrence's
                    // variadic `TString` specialization exists.  Preserve the
                    // template's Writable contract structurally across that
                    // staging seam, exactly like unmaterialized public Tuple
                    // in `is_comparable`.
                    if let Some(elements) =
                        mojito_types::types::tstring_elements(ty).or_else(|| tuple_elements(ty))
                    {
                        return elements
                            .into_iter()
                            .all(|element| self.conforms_to(element, tr));
                    }
                    match ty {
                        // The intrinsic slice descriptors write as
                        // `Slice(start, end, step)` (upstream's `Writable`).
                        Ty::Struct(name, args)
                            if args.is_empty()
                                && matches!(
                                    name.as_str(),
                                    "Slice" | "ContiguousSlice" | "StridedSlice"
                                ) =>
                        {
                            true
                        }
                        Ty::Struct(name, args) => self.struct_conformance_applies(name, args, tr),
                        Ty::Param { bounds, .. } => bounds.iter().any(|bound| bound == tr),
                        Ty::Func { .. } | Ty::GenericFunc { .. } | Ty::Overload(_) => false,
                        _ => true,
                    }
                }
                "Writer" | "Hasher" => match ty {
                    Ty::Struct(name, args) => self.struct_conformance_applies(name, args, tr),
                    Ty::Param { bounds, .. } => bounds.iter().any(|bound| bound == tr),
                    _ => false,
                },
                "Indexer" => match ty {
                    Ty::Int | Ty::IntLiteral => true,
                    Ty::Struct(name, args) => self.struct_conformance_applies(name, args, tr),
                    Ty::Param { bounds, .. } => bounds.iter().any(|bound| bound == tr),
                    _ => false,
                },
                "Equatable" => has_equality_bound_or_concrete(self, ty),
                "Comparable" => self.is_comparable(ty),
                "Absable" | "Roundable" | "Powable" | "Addable" | "Subtractable"
                | "Multipliable" | "Divisible" | "FloorDivisible" | "Modable" => {
                    is_numeric_like(ty)
                }
                "ShiftLeftable" | "ShiftRightable" | "Andable" | "Orable" | "Xorable" => {
                    is_integer_like(ty)
                }
                "Negatable" => is_signed_numeric_like(ty),
                // A struct declaring Intable (with its `__int__`) conforms
                // like the numeric scalars — integer-Scalar construction
                // accepts any Intable value.
                "Intable" => {
                    is_numeric_like(ty)
                        || *ty == Ty::Bool
                        || matches!(ty, Ty::Struct(name, args)
                            if self.struct_conformance_applies(name, args, tr))
                }
                "Floatable" => is_numeric_like(ty),
                // Layout/backend markers and future operation traits stay shallow.
                _ => true,
            };
        }
        match ty {
            Ty::Struct(name, args) => self.struct_conformance_applies(name, args, tr),
            Ty::Param { bounds, .. } => bounds
                .iter()
                .any(|bound| bound == tr || self.trait_refines(bound, tr)),
            _ => false,
        }
    }

    /// A positive `conforms_to(T, Trait)` atom from the active method's
    /// availability clause refines only that opaque parameter while its body is
    /// checked. No negative or disjunctive fact reaches this table (see
    /// `guaranteed_conformance_atoms`).
    pub(super) fn has_assumed_conformance(&self, ty: &Ty, required: &str) -> bool {
        let Ty::Param { name, .. } = ty else {
            return false;
        };
        let name = name.trim_start_matches('*');
        self.assumed_conformances.iter().rev().any(|scope| {
            scope.iter().any(|(parameter, available)| {
                parameter.trim_start_matches('*') == name
                    && (available == required
                        || self.trait_refines(available, required)
                        || matches!(
                            (available.as_str(), required),
                            ("ImplicitlyCopyable", "Copyable")
                                | (
                                    "TrivialRegisterPassable",
                                    "ImplicitlyCopyable" | "Copyable" | "Movable" | "Deinitable"
                                )
                                | ("IsTriviallyCopyable", "Copyable" | "ImplicitlyCopyable")
                                | ("IsTriviallyMovable", "Movable")
                                | ("IsTriviallyDeinitable", "Deinitable")
                        ))
            })
        })
    }

    pub(super) fn struct_conformance_applies(
        &self,
        name: &str,
        args: &[TyArg],
        required: &str,
    ) -> bool {
        let Some(info) = self.structs.get(name) else {
            return false;
        };
        info.conforms.iter().any(|declared| {
            (declared == required || self.trait_refines(declared, required))
                && info
                    .conformance_conditions
                    .get(declared)
                    .is_none_or(|condition| self.eval_conformance_condition(info, args, condition))
        })
    }

    pub(super) fn eval_conformance_condition(
        &self,
        info: &StructInfo,
        args: &[TyArg],
        expr: &Expr,
    ) -> bool {
        let arguments: HashMap<&str, &TyArg> = info
            .decls
            .iter()
            .zip(args)
            .map(|(decl, arg)| {
                let name = match decl {
                    ParamDecl::Type { name, .. } | ParamDecl::Value { name, .. } => name.as_str(),
                };
                (name, arg)
            })
            .collect();
        self.eval_conformance_predicate(expr, &arguments)
    }

    pub(super) fn eval_conformance_predicate(
        &self,
        expr: &Expr,
        args: &HashMap<&str, &TyArg>,
    ) -> bool {
        // A predicate-alias application inlines its compiled Bool body (with
        // the condition's spellings substituted), then evaluates it under the
        // same argument environment as the surrounding condition.
        if let Some((name, param_args)) = self.predicate_alias_application(expr) {
            let name = name.to_string();
            return match self.apply_predicate_alias(&name, &param_args) {
                Ok(constraint) => self.eval_generic_constraint(&constraint, args),
                Err(_) => false,
            };
        }
        match &expr.kind {
            ExprKind::TupleLit(elements)
                if matches!(
                    elements.as_slice(),
                    [
                        _,
                        Expr {
                            kind: ExprKind::Str(_),
                            ..
                        }
                    ]
                ) =>
            {
                self.eval_conformance_predicate(&elements[0], args)
            }
            ExprKind::Bool(value) => *value,
            ExprKind::TypeApply {
                name,
                args: applied,
            } if mojito_types::types::trivial_predicate_name(name).is_some()
                && applied.len() == 1 =>
            {
                let kind = mojito_types::types::trivial_predicate_name(name).expect("guarded");
                let operand = match &applied[0] {
                    mojito_ast::ast::ParamArg::Type(SourceType::Named(param, param_args))
                        if param_args.is_empty() =>
                    {
                        Some(param.as_str())
                    }
                    mojito_ast::ast::ParamArg::Value(Expr {
                        kind: ExprKind::Identifier(param),
                        ..
                    }) => Some(param.as_str()),
                    _ => None,
                };
                match operand.and_then(|param| args.get(param)) {
                    Some(TyArg::Ty(ty)) => self.is_trivially(kind, ty),
                    _ => false,
                }
            }
            // The single-bracket-argument spelling parses as runtime indexing.
            ExprKind::Index { object, index }
                if matches!(
                    &object.kind,
                    ExprKind::Identifier(name)
                        if mojito_types::types::trivial_predicate_name(name).is_some()
                ) =>
            {
                let ExprKind::Identifier(name) = &object.kind else {
                    unreachable!("guarded above");
                };
                let kind = mojito_types::types::trivial_predicate_name(name).expect("guarded");
                let param = match &index.kind {
                    ExprKind::Identifier(param) => Some(param.as_str()),
                    _ => None,
                };
                match param.and_then(|param| args.get(param)) {
                    Some(TyArg::Ty(ty)) => self.is_trivially(kind, ty),
                    _ => false,
                }
            }
            ExprKind::Prefix(PrefixOp::Not, value) => !self.eval_conformance_predicate(value, args),
            ExprKind::Infix(InfixOp::And, left, right) => {
                self.eval_conformance_predicate(left, args)
                    && self.eval_conformance_predicate(right, args)
            }
            ExprKind::Infix(InfixOp::Or, left, right) => {
                self.eval_conformance_predicate(left, args)
                    || self.eval_conformance_predicate(right, args)
            }
            ExprKind::Infix(op, left, right)
                if matches!(
                    op,
                    InfixOp::Eq
                        | InfixOp::Ne
                        | InfixOp::Lt
                        | InfixOp::Le
                        | InfixOp::Gt
                        | InfixOp::Ge
                ) =>
            {
                let Some(left) = conformance_operand(left, args) else {
                    return false;
                };
                let Some(right) = conformance_operand(right, args) else {
                    return false;
                };
                compare_ct_integers(*op, &left, &right).unwrap_or_else(|| match op {
                    InfixOp::Eq => ct_values_equal(&left, &right),
                    InfixOp::Ne => !ct_values_equal(&left, &right),
                    _ => false,
                })
            }
            ExprKind::Call {
                name,
                args: operands,
                kwargs,
                ..
            } if name == "conforms_to" && kwargs.is_empty() && operands.len() == 2 => {
                let ExprKind::Identifier(type_name) = &operands[0].kind else {
                    return false;
                };
                let ExprKind::Identifier(trait_name) = &operands[1].kind else {
                    return false;
                };
                let trait_name = mojito_ast::ast::canonical_trait_name(trait_name);
                matches!(args.get(type_name.as_str()), Some(TyArg::Ty(ty)) if self.conforms_to(ty, trait_name))
            }
            _ => false,
        }
    }

    pub(super) fn trait_refines(&self, candidate: &str, required: &str) -> bool {
        self.trait_refines_inner(candidate, required, &mut HashSet::new())
    }

    pub(super) fn trait_refines_inner(
        &self,
        candidate: &str,
        required: &str,
        visiting: &mut HashSet<String>,
    ) -> bool {
        if !visiting.insert(candidate.to_string()) {
            return false;
        }
        self.traits.get(candidate).is_some_and(|info| {
            info.refines.iter().any(|parent| {
                parent == required || self.trait_refines_inner(parent, required, visiting)
            })
        })
    }

    /// Explain the first actionable reason a built-in bound failed. This is
    /// intentionally evidence-oriented: marker traits name the field that
    /// prevents fieldwise synthesis, while operation traits name the operation
    /// promised by the bound.
    pub(super) fn trait_failure_reason(&self, ty: &Ty, tr: &str) -> Option<String> {
        let Ty::Struct(name, arguments) = ty else {
            return builtin_trait_operation(tr)
                .map(|operation| format!("missing required operation '{operation}'"));
        };
        let info = self.structs.get(name)?;
        for declared in &info.conforms {
            if declared != tr && !self.trait_refines(declared, tr) {
                continue;
            }
            let Some(condition) = info.conformance_conditions.get(declared) else {
                continue;
            };
            if self.eval_conformance_condition(info, arguments, condition) {
                continue;
            }
            if let ExprKind::TupleLit(elements) = &condition.kind
                && let [
                    _,
                    Expr {
                        kind: ExprKind::Str(message),
                        ..
                    },
                ] = elements.as_slice()
            {
                return Some(message.clone());
            }
        }
        let field_failure = |predicate: &dyn Fn(&Ty) -> bool| {
            info.fields
                .iter()
                .find(|(_, field_ty)| !predicate(field_ty))
                .map(|(field, field_ty)| {
                    format!("field '{field}' has type '{field_ty}', which is not {tr}")
                })
        };
        match tr {
            "Hashable" => {
                let hashes = info.methods.get("__hash__");
                if !info
                    .conforms
                    .iter()
                    .any(|conformance| conformance == "Hashable")
                {
                    Some(format!("'{name}' does not declare Hashable conformance"))
                } else if hashes.is_some_and(|methods| {
                    methods
                        .iter()
                        .any(|method| method.params.is_empty() && method.ret == Ty::UInt)
                }) {
                    Some(
                        "'__hash__(self) -> UInt' is not the Hashable protocol; spell \
                         'def __hash__(self, mut hasher: Some[Hasher])' (or \
                         '[H: Hasher](self, mut hasher: H)') and feed the hasher with \
                         'hasher.update(...)'"
                            .to_string(),
                    )
                } else if hashes.is_none() {
                    field_failure(&|field_ty| self.is_hashable(field_ty))
                } else {
                    Some(
                        "missing required operation '__hash__(self, mut hasher: Some[Hasher]) -> None'"
                            .to_string(),
                    )
                }
            }
            "Hasher" => {
                let has = |method: &str| info.methods.contains_key(method);
                ["__init__", "_update_with_bytes", "_update_with_simd", "update", "finish"]
                    .into_iter()
                    .find(|method| !has(method))
                    .map(|method| format!("missing required Hasher member '{method}'"))
                    .or_else(|| {
                        Some(
                            "a Hasher member has the wrong shape (expected '_update_with_simd(mut self, UInt64)', \
                             '_update_with_bytes(mut self, Span[Byte, _])', 'update(mut self, Some[Hashable])', \
                             and 'finish(var self) -> UInt64')"
                                .to_string(),
                        )
                    })
            }
            "Copyable" => field_failure(&|field_ty| self.is_copyable(field_ty)),
            "ImplicitlyCopyable" => {
                field_failure(&|field_ty| self.is_implicitly_copyable(field_ty))
            }
            "Deinitable" => field_failure(&|field_ty| self.is_deinitable(field_ty)),
            "Movable" => info
                .conforms
                .iter()
                .any(|conformance| conformance == "Movable")
                .then(|| {
                    "its declared 'Movable' conformance condition evaluates to false".to_string()
                }),
            _ => builtin_trait_operation(tr)
                .map(|operation| format!("missing required operation '{operation}'")),
        }
    }

    /// Whether a value of this type may be **copied** (implicitly duplicated).
    /// Mojo is move-only by default: scalars and the built-in value types are
    /// Copyable, but a `struct` is Copyable only if it declares Copyable/
    /// ImplicitlyCopyable conformance **or defines `__copyinit__`**, and a type
    /// parameter only if bounded by Copyable/ImplicitlyCopyable.
    pub(super) fn is_copyable(&self, ty: &Ty) -> bool {
        if self.has_assumed_conformance(ty, "Copyable")
            || self.has_assumed_conformance(ty, "ImplicitlyCopyable")
        {
            return true;
        }
        // A lazy TString is declared Movable + Writable only.  Answer
        // structurally so a not-yet-materialized specialization (a nested
        // t-string element during discovery) never falls back to the
        // permissive unregistered-struct default below.
        if mojito_types::types::tstring_elements(ty).is_some() {
            return false;
        }
        match ty {
            Ty::ComptimeList(element) => self.is_copyable(element),
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => {
                elements.iter().all(|element| self.is_copyable(element))
            }
            Ty::Variant(alternatives) => alternatives
                .iter()
                .all(|alternative| self.is_copyable(alternative)),
            Ty::Struct(name, args) => self
                .structs
                .get(name)
                .map(|s| {
                    s.conforms.iter().any(|c| {
                        matches!(c.as_str(), "Copyable" | "ImplicitlyCopyable")
                            && s.conformance_conditions.get(c).is_none_or(|condition| {
                                self.eval_conformance_condition(s, args, condition)
                            })
                    }) || s.methods.contains_key("__copyinit__")
                })
                .unwrap_or(true),
            Ty::Param { bounds, .. } => bounds.iter().any(|bound| {
                matches!(
                    bound.as_str(),
                    "Copyable" | "ImplicitlyCopyable" | "TrivialRegisterPassable"
                ) || self.trait_refines(bound, "Copyable")
            }),
            // An abstract associated type (`C.Element`) is copyable only when
            // the bound trait's member declaration proves it — never
            // vacuously.
            Ty::Assoc { .. } => self.assoc_member_bound_proves(ty, "Copyable"),
            // Scalars, `String`, `List`/`Tuple`/`Simd`/`Range`, `Error`, closures,
            // and `Self` are treated as copyable (element-wise copyability of
            // aggregates is not modeled).
            _ => true,
        }
    }

    /// `ImplicitlyCopyable` is stronger than `Copyable`: it means the type can be
    /// copied by the ordinary implicit copy path, not only by an explicit custom
    /// copy constructor. Structs opt in by declaring the marker, and fieldwise
    /// conformance requires all fields to be implicitly copyable.
    pub(super) fn is_implicitly_copyable(&self, ty: &Ty) -> bool {
        if self.has_assumed_conformance(ty, "ImplicitlyCopyable") {
            return true;
        }
        // Structural, like `is_copyable`: a lazy TString never copies.
        if mojito_types::types::tstring_elements(ty).is_some() {
            return false;
        }
        // Compiler-private inline uninit storage copies its raw bits; the
        // `MaybeUninit` header conditions gate the public copy on the
        // payload's triviality.
        if mojito_types::types::uninit_storage_element(ty).is_some() {
            return true;
        }
        match ty {
            Ty::ComptimeList(element) => self.is_implicitly_copyable(element),
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => elements
                .iter()
                .all(|element| self.is_implicitly_copyable(element)),
            Ty::Variant(alternatives) => alternatives
                .iter()
                .all(|alternative| self.is_implicitly_copyable(alternative)),
            Ty::Struct(name, args) => self.structs.get(name).map_or_else(
                || {
                    mojito_types::types::list_element(ty).is_none()
                        && mojito_types::types::dict_elements(ty).is_none()
                        && mojito_types::types::set_element(ty).is_none()
                        && mojito_types::types::optional_element(ty).is_none()
                        && mojito_types::types::array_element(ty).is_none()
                        && mojito_types::types::owned_pointer_element(ty).is_none()
                },
                |s| {
                    s.conforms.iter().any(|c| {
                        c == "ImplicitlyCopyable"
                            && s.conformance_conditions.get(c).is_none_or(|condition| {
                                self.eval_conformance_condition(s, args, condition)
                            })
                    }) && self.struct_implicitly_copyable_conformance_ok(name)
                },
            ),
            Ty::Param { bounds, .. } => bounds.iter().any(|bound| {
                matches!(
                    bound.as_str(),
                    "ImplicitlyCopyable" | "TrivialRegisterPassable"
                ) || self.trait_refines(bound, "ImplicitlyCopyable")
            }),
            // An abstract associated type is implicitly copyable only when its
            // declared member bounds say so, mirroring the `Ty::Param` rule.
            Ty::Assoc { .. } => self.assoc_member_bound_proves(ty, "ImplicitlyCopyable"),
            _ => true,
        }
    }

    /// Whether a shared call argument is materialized independently before
    /// within-call exclusivity is checked. Upstream keeps nominal memory values
    /// (including `String`) as overlapping reads even when their type conforms
    /// to `ImplicitlyCopyable`; register-like values may be disarmed as copies.
    pub(super) fn call_read_is_independent_copy(&self, ty: &Ty) -> bool {
        self.is_implicitly_copyable(ty) && !matches!(ty, Ty::Struct(..))
    }

    /// Whether an associated type projected off a bounded parameter
    /// (`C.Element`) proves `required` through the bound trait's declared
    /// member bounds. `Copyable` is satisfied by `Copyable`,
    /// `ImplicitlyCopyable`, or a refining trait; `ImplicitlyCopyable`
    /// requires the exact marker, matching the `Ty::Param` rule.
    fn assoc_member_bound_proves(&self, ty: &Ty, required: &str) -> bool {
        let Ty::Assoc { base, name, .. } = ty else {
            return false;
        };
        let Ty::Param { bounds, .. } = base.as_ref() else {
            return false;
        };
        self.lookup_trait_assoc_type(bounds, name)
            .is_some_and(|associated_bounds| {
                associated_bounds.iter().any(|bound| {
                    bound == required
                        || (required == "Copyable"
                            && (bound == "ImplicitlyCopyable"
                                || self.trait_refines(bound, "Copyable")))
                })
            })
    }

    /// Every initialized value is movable by default; only a struct that
    /// *declares* a conditional `Movable` conformance whose predicate fails
    /// (commonly `Movable where False`) opts out. `Ty::Param` stays movable
    /// without a `Movable` bound — a deliberate asymmetry with
    /// `is_deinitable`: the default-movable model means generic code moves
    /// unbounded parameters, and only declared opt-outs are enforced.
    pub(super) fn is_movable(&self, ty: &Ty) -> bool {
        if self.has_assumed_conformance(ty, "Movable") {
            return true;
        }
        match ty {
            Ty::ComptimeList(element) => self.is_movable(element),
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => {
                elements.iter().all(|element| self.is_movable(element))
            }
            Ty::Variant(alternatives) => alternatives
                .iter()
                .all(|alternative| self.is_movable(alternative)),
            Ty::Struct(name, args) => self.structs.get(name).is_none_or(|info| {
                if info.conforms.iter().any(|tr| tr == "Movable") {
                    self.struct_conformance_applies(name, args, "Movable")
                } else {
                    true
                }
            }),
            _ => true,
        }
    }

    /// The `IsTrivially{Movable,Copyable,Deinitable}[T]` predicate: the type
    /// conforms to `TrivialRegisterPassable`, OR the base capability holds AND
    /// the corresponding lifecycle operation is compiler-generated (no user
    /// `__moveinit__`/`__copyinit__`/`__deinit__` or named destructor for the
    /// queried facet) AND every field is recursively trivial — a bitwise
    /// move/copy or a no-op destructor, matching upstream `std.traits`.
    pub(super) fn is_trivially(
        &self,
        kind: mojito_types::types::TrivialLifecycle,
        ty: &Ty,
    ) -> bool {
        let mut visiting = std::collections::HashSet::new();
        self.trivial_lifecycle(kind, ty, &mut visiting)
    }

    fn trivial_lifecycle(
        &self,
        kind: mojito_types::types::TrivialLifecycle,
        ty: &Ty,
        visiting: &mut std::collections::HashSet<String>,
    ) -> bool {
        use mojito_types::types::TrivialLifecycle;
        // A `where IsTrivially*[T]` clause on the enclosing body guarantees
        // the facet for the parameter it names.
        if self.has_assumed_conformance(ty, mojito_types::types::trivial_predicate_spelling(kind)) {
            return true;
        }
        // The first upstream disjunct: `conforms_to(T, TrivialRegisterPassable)`.
        // The general `conforms_to` treats marker traits shallowly (everything
        // conforms), so consult only a declared conformance or a parameter
        // bound; primitives already prove triviality structurally below.
        let trp_conforms = match ty {
            Ty::Struct(name, args) => {
                self.struct_conformance_applies(name, args, "TrivialRegisterPassable")
            }
            Ty::Param { bounds, .. } => {
                bounds
                    .iter()
                    .any(|bound| bound == "TrivialRegisterPassable")
                    || self.has_assumed_conformance(ty, "TrivialRegisterPassable")
            }
            _ => false,
        };
        if trp_conforms {
            return true;
        }
        let base_holds = match kind {
            TrivialLifecycle::Movable => self.is_movable(ty),
            TrivialLifecycle::Copyable => self.is_copyable(ty),
            TrivialLifecycle::Deinitable => self.is_deinitable(ty),
        };
        if !base_holds {
            return false;
        }
        // Compiler-private inline uninit storage: its destructor is always a
        // no-op (the payload deliberately leaks), while move/copy triviality
        // is the payload's — matching upstream `MaybeUninit`'s comptime
        // lifecycle facts.
        if let Some(element) = mojito_types::types::uninit_storage_element(ty) {
            return match kind {
                TrivialLifecycle::Deinitable => true,
                TrivialLifecycle::Movable | TrivialLifecycle::Copyable => {
                    self.trivial_lifecycle(kind, element, visiting)
                }
            };
        }
        match ty {
            Ty::ComptimeList(element) => self.trivial_lifecycle(kind, element, visiting),
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => elements
                .iter()
                .all(|element| self.trivial_lifecycle(kind, element, visiting)),
            Ty::Variant(alternatives) => alternatives
                .iter()
                .all(|alternative| self.trivial_lifecycle(kind, alternative, visiting)),
            Ty::Struct(name, args) => {
                if !visiting.insert(name.clone()) {
                    // A cycle can only occur through indirection; the pointer
                    // value itself is trivial.
                    return true;
                }
                let result = self.structs.get(name).is_some_and(|info| {
                    let user_defeats = match kind {
                        TrivialLifecycle::Movable => info.methods.contains_key("__moveinit__"),
                        TrivialLifecycle::Copyable => info.methods.contains_key("__copyinit__"),
                        TrivialLifecycle::Deinitable => {
                            info.methods.contains_key("__deinit__")
                                || !info.explicit_destructors.is_empty()
                        }
                    };
                    // Fields are stored at the declaration's parameters;
                    // recurse at this instantiation's arguments so a generic
                    // payload field answers for the concrete element.
                    !user_defeats
                        && info.fields.iter().all(|(_, field_ty)| {
                            let field_ty = substitute_at(field_ty, &info.decls, args);
                            self.trivial_lifecycle(kind, &field_ty, visiting)
                        })
                });
                visiting.remove(name);
                result
            }
            // Generic parameters and associated members have no structural
            // proof; comptime sites see concrete types after specialization.
            Ty::Param { .. } | Ty::SelfType | Ty::Infer => false,
            // Scalars, literals, pointers-as-values, and the remaining
            // primitive representations move/copy bitwise and drop as no-ops.
            _ => true,
        }
    }

    pub(super) fn is_deinitable(&self, ty: &Ty) -> bool {
        if self.has_assumed_conformance(ty, "Deinitable") {
            return true;
        }
        match ty {
            Ty::ComptimeList(element) => self.is_deinitable(element),
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => {
                elements.iter().all(|element| self.is_deinitable(element))
            }
            Ty::Variant(alternatives) => alternatives
                .iter()
                .all(|alternative| self.is_deinitable(alternative)),
            Ty::Struct(name, args) => self.structs.get(name).is_none_or(|info| {
                if info.conforms.iter().any(|tr| tr == "Deinitable") {
                    self.struct_conformance_applies(name, args, "Deinitable")
                } else {
                    true
                }
            }),
            Ty::Param { bounds, .. } => bounds.iter().any(|b| b == "Deinitable"),
            _ => true,
        }
    }

    pub(super) fn is_hashable(&self, ty: &Ty) -> bool {
        if self.has_assumed_conformance(ty, "Hashable") {
            return true;
        }
        // Same discovery-staging seam as `is_comparable`: a public Tuple's
        // conditional Hashable contract is evaluated structurally until its
        // concrete specialization replaces the variadic template.
        if let Some(elements) = tuple_elements(ty) {
            return elements
                .into_iter()
                .all(|element| self.is_hashable(element));
        }
        match ty {
            Ty::Struct(name, args) => self.struct_conformance_applies(name, args, "Hashable"),
            Ty::Param { bounds, .. } => bounds.iter().any(|b| b == "Hashable"),
            _ => builtin_hashable_ty(ty),
        }
    }

    pub(super) fn is_comparable(&self, ty: &Ty) -> bool {
        if self.has_assumed_conformance(ty, "Comparable") {
            return true;
        }
        // The discovery check runs before variadic public-Tuple templates have
        // been replaced by concrete generated declarations. Preserve the
        // template's conditional Comparable contract structurally across that
        // staging seam; the final specialization carries the same conformance
        // as an ordinary nominal declaration.
        if let Some(elements) = tuple_elements(ty) {
            return elements
                .into_iter()
                .all(|element| self.is_comparable(element));
        }
        match ty {
            Ty::Struct(name, args) => self.struct_conformance_applies(name, args, "Comparable"),
            Ty::Param { bounds, .. } => bounds.iter().any(|b| b == "Comparable"),
            _ => is_numeric_like(ty),
        }
    }

    pub(super) fn struct_copyable_conformance_ok(&self, name: &str) -> bool {
        let Some(info) = self.structs.get(name) else {
            return false;
        };
        info.methods.contains_key("__copyinit__")
            || info.fields.iter().all(|(_, ty)| self.is_copyable(ty))
    }

    pub(super) fn struct_implicitly_copyable_conformance_ok(&self, name: &str) -> bool {
        let Some(info) = self.structs.get(name) else {
            return false;
        };
        info.methods.contains_key("__copyinit__")
            || info
                .fields
                .iter()
                .all(|(_, ty)| self.is_implicitly_copyable(ty))
    }

    /// At a **consuming** position (binding a value to a new place, passing it by
    /// value, returning it, …): a non-Copyable value that is a *place* (names an
    /// existing binding) is being copied — reject it unless it was transferred with
    /// `^` (which is a move, not a place). `context` names the site for the error.
    pub(super) fn check_consuming(
        &self,
        expr: &Expr,
        ty: &Ty,
        context: &str,
    ) -> Result<(), TypeError> {
        self.check_consuming_as(expr, ty, context, ConsumeKind::Move)
    }

    pub(super) fn check_consuming_as(
        &self,
        expr: &Expr,
        ty: &Ty,
        context: &str,
        kind: ConsumeKind,
    ) -> Result<(), TypeError> {
        // A `^` transfer is an ownership move: gated on `Movable`, so a
        // declared conditional opt-out (`Movable where False`) rejects here.
        // A `deinit` binding is consumption-for-destruction, not a move — a
        // non-Movable value must remain destructible by its own destructor.
        let mut source = expr;
        while let ExprKind::Named { value, .. } = &source.kind {
            source = value;
        }
        if kind == ConsumeKind::Move
            && matches!(source.kind, ExprKind::Transfer(_))
            && !self.is_movable(ty)
        {
            return Err(TypeError::TraitNotSatisfied {
                param: context.to_string(),
                ty: ty.to_string(),
                trait_name: "Movable".to_string(),
                reason: self
                    .trait_failure_reason(ty, "Movable")
                    .or_else(|| Some("its 'Movable' conformance condition is false".to_string())),
            });
        }
        // A transfer of a Movable value and a fresh temporary (a call result,
        // a literal, an operator) move freely; a *place* must be Copyable.
        if is_place_expr(expr) {
            if !self.is_copyable(ty) {
                return Err(TypeError::NonCopyable {
                    ty: ty.to_string(),
                    context: context.to_string(),
                });
            }
            if !self.is_implicitly_copyable(ty) {
                let transferable = self.is_movable(ty)
                    && super::places::place_path(expr)
                        .is_some_and(|(root, _)| self.is_binding_mutable(root));
                return Err(TypeError::ImplicitCopy {
                    ty: ty.to_string(),
                    context: context.to_string(),
                    transferable,
                    copyable: true,
                });
            }
            self.copy_place_value_uses
                .borrow_mut()
                .insert(expr.source_span());
        }
        Ok(())
    }

    /// Find every `method` required by the given trait `bounds`. Keeping the
    /// full candidate set is important: bounded calls use the same named-argument
    /// binder, generic specialization, overload ranking, and effect selection as
    /// concrete method calls.
    pub(super) fn lookup_trait_methods(
        &self,
        bounds: &[String],
        method: &str,
        argc: usize,
    ) -> Vec<MethodSig> {
        let mut methods = Vec::new();
        if method == "__hash__" && argc == 1 && bounds.iter().any(|b| b == "Hashable") {
            let mut signature = MethodSig::intrinsic(
                vec![Ty::Param {
                    name: "Some[Hasher]".to_string(),
                    bounds: vec!["Hasher".to_string()],
                    callable_bound: None,
                }],
                Ty::None,
            );
            signature.conventions[0] = Some(ArgConvention::Mut);
            methods.push(signature);
        }
        if bounds.iter().any(|bound| bound == "Hasher") {
            let signature = match (method, argc) {
                ("update", 1) => Some((
                    Ty::Param {
                        name: "Some[Hashable]".to_string(),
                        bounds: vec!["Hashable".to_string()],
                        callable_bound: None,
                    },
                    Ty::None,
                    Some(ArgConvention::Mut),
                )),
                ("_update_with_simd", 1) => Some((
                    Ty::Simd {
                        dtype: mojito_ast::ast::Dtype::UInt64,
                        width: 1,
                    },
                    Ty::None,
                    Some(ArgConvention::Mut),
                )),
                _ => None,
            };
            if let Some((param, ret, convention)) = signature {
                let mut signature = MethodSig::intrinsic(vec![param], ret);
                signature.self_convention = convention;
                methods.push(signature);
            }
            if method == "finish" && argc == 0 {
                let mut signature = MethodSig::intrinsic(
                    vec![],
                    Ty::Simd {
                        dtype: mojito_ast::ast::Dtype::UInt64,
                        width: 1,
                    },
                );
                signature.self_convention = Some(ArgConvention::Var);
                methods.push(signature);
            }
        }
        // Current Mojo's `Copyable` trait carries a non-overridable default
        // `copy(self) -> Self`; elaboration synthesizes the concrete method on
        // every conforming struct, and a Copyable-bounded parameter resolves
        // the same contract here.
        if method == "copy"
            && argc == 0
            && bounds
                .iter()
                .any(|b| matches!(b.as_str(), "Copyable" | "ImplicitlyCopyable"))
        {
            methods.push(MethodSig::intrinsic(vec![], Ty::SelfType));
        }
        // The built-in numeric-rounding traits contribute a `-> Self` dunder
        // (roadmap milestone 7), used by the self-hosted `math` module: `Floorable`/
        // `Ceilable`/`Truncable` a nullary `__floor__`/`__ceil__`/`__trunc__`,
        // and `CeilDivable`/`CeilDivableRaising` a unary `__ceildiv__(Self)`.
        let accepts = math_dunder_bound(method, argc);
        if !accepts.is_empty() && bounds.iter().any(|b| accepts.contains(&b.as_str())) {
            let params = if argc == 1 {
                vec![Ty::SelfType]
            } else {
                vec![]
            };
            methods.push(MethodSig::intrinsic(params, Ty::SelfType));
        }
        for bound in bounds {
            let Some(signatures) = self
                .traits
                .get(bound)
                .and_then(|info| info.methods.get(method))
            else {
                continue;
            };
            for signature in signatures {
                if !methods.contains(signature) {
                    methods.push(signature.clone());
                }
            }
        }
        methods
    }

    /// Find a type-valued associated comptime member required by any of the
    /// given trait bounds. Built-in bounds contribute none.
    pub(super) fn lookup_trait_assoc_type(
        &self,
        bounds: &[String],
        member: &str,
    ) -> Option<Vec<String>> {
        bounds
            .iter()
            .filter_map(|b| self.traits.get(b))
            .find_map(|info| match info.comptime_members.get(member) {
                Some(CtMemberReq::Type { bounds, .. }) => Some(bounds.clone()),
                _ => None,
            })
    }

    /// The parameter list of a parameterized associated type required by one of
    /// `bounds`, or `None` if the member is monomorphic or absent.
    pub(super) fn lookup_trait_assoc_params(
        &self,
        bounds: &[String],
        member: &str,
    ) -> Option<Vec<mojito_ast::ast::TypeParam>> {
        bounds
            .iter()
            .filter_map(|b| self.traits.get(b))
            .find_map(|info| match info.comptime_members.get(member) {
                Some(CtMemberReq::Type { params, .. }) if !params.is_empty() => {
                    Some(params.clone())
                }
                _ => None,
            })
    }

    /// Find a value-valued associated comptime member required by a bound trait.
    pub(super) fn lookup_trait_assoc_value_ty(
        &self,
        bounds: &[String],
        member: &str,
    ) -> Option<Ty> {
        bounds
            .iter()
            .filter_map(|b| self.traits.get(b))
            .find_map(|info| match info.comptime_members.get(member) {
                Some(CtMemberReq::Value(ty)) => Some((**ty).clone()),
                _ => None,
            })
    }
}

/// Origin parameters a method body writes through via a parametric-mut ref
/// field subscript (`self.<field>[...] = v` / `+=`). The scan is syntactic
/// and complete for the accepted subset: only the direct self-rooted field
/// subscript form is a legal write target through such a field (alias forms
/// stay rejected by the reborrow rules), so no checked facts are needed.
fn parametric_origin_writes_in_body(
    body: &[mojito_ast::ast::Stmt],
    fields: &[(String, Ty)],
) -> Vec<mojito_types::origin::OriginParamId> {
    use mojito_ast::ast::{ExprKind, Stmt, StmtKind};
    use mojito_types::origin::{Mutability, Origin};

    fn written_self_field(place: &mojito_ast::ast::Expr) -> Option<&str> {
        let ExprKind::Index { object, .. } = &place.kind else {
            return None;
        };
        let ExprKind::Member { object, field } = &object.kind else {
            return None;
        };
        matches!(&object.kind, ExprKind::Identifier(name) if name == "self")
            .then_some(field.as_str())
    }

    fn walk(
        stmts: &[Stmt],
        fields: &[(String, Ty)],
        out: &mut Vec<mojito_types::origin::OriginParamId>,
    ) {
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::AugAssign { place, .. } | StmtKind::SetPlace { place, value: _ } => {
                    if let Some(field) = written_self_field(place)
                        && let Some((_, Ty::Ref(reference))) =
                            fields.iter().find(|(name, _)| name == field)
                        && matches!(reference.mutability, Mutability::Param(_))
                        && let Origin::Param(id) = reference.origin
                        && !out.contains(&id)
                    {
                        out.push(id);
                    }
                }
                StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
                    for (_, branch) in branches {
                        walk(branch, fields, out);
                    }
                    if let Some(branch) = orelse {
                        walk(branch, fields, out);
                    }
                }
                StmtKind::While { body, orelse, .. } | StmtKind::For { body, orelse, .. } => {
                    walk(body, fields, out);
                    if let Some(branch) = orelse {
                        walk(branch, fields, out);
                    }
                }
                StmtKind::ComptimeFor { body, .. } | StmtKind::With { body, .. } => {
                    walk(body, fields, out);
                }
                StmtKind::Try {
                    body,
                    except,
                    orelse,
                    finalbody,
                } => {
                    walk(body, fields, out);
                    if let Some((_, handler)) = except {
                        walk(handler, fields, out);
                    }
                    if let Some(branch) = orelse {
                        walk(branch, fields, out);
                    }
                    if let Some(branch) = finalbody {
                        walk(branch, fields, out);
                    }
                }
                _ => {}
            }
        }
    }

    let mut out = Vec::new();
    walk(body, fields, &mut out);
    out
}

/// The outer checker scope saved while a struct's members resolve at the
/// struct's own type parameters; restored by `Checker::exit_struct_scope`.
struct SavedStructScope {
    forward_types: bool,
    type_params: Vec<mojito_ast::ast::TypeParam>,
    self_decls: Vec<ParamDecl>,
    self_ty: Option<Ty>,
    bundled_stdlib: bool,
}

/// The resolved member types of one struct declaration: fields, associated
/// values, parameterized associated members, and the callable conformance.
type StructMemberTypes = (
    Vec<(String, Ty)>,
    HashMap<String, Vec<(u32, u32)>>,
    HashMap<String, CtValue>,
    HashMap<String, Vec<GenericConstraint>>,
    HashMap<String, ParameterizedMember>,
    Option<Ty>,
);

//! Trait and struct declaration checking, trait conformance (nominal and
//! built-in), type-capability queries (Copyable/Movable/Hashable/…), and
//! trait-method/associated-member lookup. Extracted from `checker.rs`;
//! see `docs/symbol-map.md`.

use super::*;

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
        methods: &[crate::ast::TraitMethod],
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
        }
        for member in comptime_members {
            let requirement = self.ct_member_req_from_anno(&member.params, &member.ty)?;
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
                self.validate_origin_signature(&[], &m.params, m.self_origin.as_ref())?;
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
                if let Some(condition) = &m.where_clause {
                    let constraint = self.compile_generic_constraint(condition)?;
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
                    .position(|param| param.kind == crate::ast::ParamKind::KwVariadic);
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
                    .filter(|(_, param)| param.kind == crate::ast::ParamKind::Regular)
                    .collect();
                let regular_params: Vec<_> = regular.iter().map(|(_, param)| *param).collect();
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
                    ref_params: lower_ref_param_sigs(&m.type_params, &regular_params)?,
                    ref_return: None,
                    implicit: false,
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
            },
        );
        Ok(())
    }

    /// Register a struct and check its method bodies. A generic struct's type
    /// parameters are validated and kept in scope (as `Self.T`) for its fields
    /// and methods; field/method types referring to them become `Ty::Param`.
    /// Declared trait conformances are verified once the members are known.
    pub(super) fn check_struct(
        &mut self,
        declaration: &StructDeclaration<'_>,
    ) -> Result<(), TypeError> {
        let name = declaration.name;
        let type_params = declaration.type_params;
        let conforms = declaration.conforms;
        if self.structs.contains_key(name) || self.traits.contains_key(name) {
            return Err(TypeError::Redeclaration(name.to_string()));
        }
        let decls = self.classify_params(type_params)?;
        self.generic_parameters.borrow_mut().insert(
            crate::checked::GenericSite::Struct {
                module: declaration.module.clone(),
                declaration: name.to_string(),
            },
            decls.clone(),
        );
        // A variadic struct template is compiled by compile-time specialization
        // (each instantiation is a concrete struct); the unspecialized template
        // has pack-dependent members and cannot be checked erased.
        if decls.iter().any(|decl| {
            matches!(
                decl,
                ParamDecl::Type { variadic: true, .. } | ParamDecl::Value { variadic: true, .. }
            )
        }) {
            return Err(TypeError::Unsupported(format!(
                "variadic struct '{name}' is compiled by compile-time specialization; instantiate it with explicit compile-time arguments (e.g. `{name}[Int, Bool](...)`) instead of checking the template"
            )));
        }
        for tr in conforms {
            self.check_trait_name(tr)?;
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
        let fixed_arguments = match self.generated_tuple_arguments(name, declaration.associated) {
            Ok(arguments) => arguments,
            Err(error) => {
                self.enclosing_type_params = saved_type_params;
                self.allow_generated_tuple_forward_types = saved_forward_types;
                return Err(error);
            }
        };

        // The struct's parameters are in scope as `Self.T` / `Self.n`, and bare
        // `Self` is the struct type, while checking its members. Type parameters
        // appear as `Ty::Param`, value parameters as symbolic `CtValue::Param`.
        let self_ty = Ty::Struct(
            name.to_string(),
            fixed_arguments
                .clone()
                .unwrap_or_else(|| decls.iter().map(param_as_arg).collect()),
        );
        let saved_self_decls = std::mem::replace(&mut self.self_decls, decls.clone());
        let saved_self_ty = self.self_ty.replace(self_ty.clone());
        let result = self.check_struct_members(declaration, decls, fixed_arguments, &self_ty);
        self.self_decls = saved_self_decls;
        self.enclosing_type_params = saved_type_params;
        self.self_ty = saved_self_ty;
        self.allow_generated_tuple_forward_types = saved_forward_types;
        result
    }

    pub(super) fn check_struct_members(
        &mut self,
        declaration: &StructDeclaration<'_>,
        decls: Vec<ParamDecl>,
        fixed_arguments: Option<Vec<TyArg>>,
        self_ty: &Ty,
    ) -> Result<(), TypeError> {
        let name = declaration.name;
        let conforms = declaration.conforms;
        let fields = declaration.fields;
        let associated = declaration.associated;
        let methods = declaration.methods;
        let fieldwise_init = declaration.fieldwise_init;
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
        let explicit_destructors = methods
            .iter()
            .filter(|method| {
                method.name != "__del__" && method.self_convention == Some(ArgConvention::Deinit)
            })
            .map(|method| (method.name.clone(), method.raises))
            .collect::<HashMap<_, _>>();
        // Field types are resolved against structs defined *so far* (so a struct
        // can't contain itself); duplicate field names are a redeclaration.
        let mut field_tys: Vec<(String, Ty)> = Vec::new();
        for (field_index, f) in fields.iter().enumerate() {
            if field_tys.iter().any(|(n, _)| n == &f.name) {
                return Err(TypeError::Redeclaration(f.name.clone()));
            }
            let ty = self.ty_from_anno(&f.ty)?;
            if Self::type_contains_unsafe_any_pointer(&ty) {
                return Err(TypeError::Unsupported(format!(
                    "field '{}' cannot hide a MutUnsafeAnyOrigin or ImmutUnsafeAnyOrigin pointer",
                    f.name
                )));
            }
            self.declaration_types.borrow_mut().insert(
                crate::checked::AnnotationSite::StructField {
                    module: declaration.module.clone(),
                    declaration: name.to_string(),
                    field: field_index,
                },
                ty.clone(),
            );
            field_tys.push((f.name.clone(), ty));
        }
        let associated_values = self.check_struct_associated(associated)?;
        let callable_conformance = declaration
            .callable_conformance
            .as_ref()
            .map(|annotation| self.ty_from_anno(annotation))
            .transpose()?;
        if callable_conformance
            .as_ref()
            .is_some_and(|ty| !matches!(ty, Ty::Func { .. }))
        {
            return Err(TypeError::Unsupported(
                "callable conformance must be a def(...) function type".to_string(),
            ));
        }
        // Register the (method-less) struct first, so methods may reference the
        // struct's own type (even parameterized, `Pair[Self.T]`) in signatures.
        self.structs.insert(
            name.to_string(),
            StructInfo {
                decls,
                fixed_arguments,
                conforms: conforms.to_vec(),
                callable_conformance,
                callable_target: None,
                conformance_conditions: declaration
                    .conformance_conditions
                    .iter()
                    .cloned()
                    .collect(),
                fields: field_tys,
                associated: associated_values,
                methods: HashMap::new(),
                fieldwise_init,
                explicit_destroy_message,
                explicit_destructors,
            },
        );
        // Method signatures.
        for (method_index, m) in methods.iter().enumerate() {
            let method_name = lifecycle_method_name(m);
            let method_decls = self.classify_params(&m.type_params)?;
            self.generic_parameters.borrow_mut().insert(
                crate::checked::GenericSite::Method {
                    module: declaration.module.clone(),
                    declaration: name.to_string(),
                    method: method_index,
                },
                method_decls.clone(),
            );
            self.tparams.push(type_scope(&method_decls));
            let saved_method_type_params = self.enclosing_type_params.clone();
            self.enclosing_type_params.extend(m.type_params.clone());
            let signature = (|| {
                let all_types = self.param_tys(&m.params)?;
                let sig = self.method_sig(m, method_decls, &all_types)?;
                Ok::<_, TypeError>((all_types, sig))
            })();
            self.enclosing_type_params = saved_method_type_params;
            self.tparams.pop();
            let (all_types, sig) = signature?;
            for (param, ty) in all_types.iter().enumerate() {
                self.declaration_types.borrow_mut().insert(
                    crate::checked::AnnotationSite::MethodParam {
                        module: declaration.module.clone(),
                        declaration: name.to_string(),
                        method: method_index,
                        param,
                    },
                    ty.clone(),
                );
            }
            self.declaration_types.borrow_mut().insert(
                crate::checked::AnnotationSite::MethodReturn {
                    module: declaration.module.clone(),
                    declaration: name.to_string(),
                    method: method_index,
                },
                sig.ret.clone(),
            );
            self.declaration_effects.borrow_mut().insert(
                crate::checked::AnnotationSite::MethodReturn {
                    module: declaration.module.clone(),
                    declaration: name.to_string(),
                    method: method_index,
                },
                crate::checked::DeclarationEffect {
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
                    && (method_name != "__iter__"
                        || existing.self_convention == sig.self_convention)
            }) {
                return Err(TypeError::Redeclaration(method_name.to_string()));
            }
            overloads.push(sig);
        }
        // `@fieldwise_init` and a hand-written `__init__` both define a constructor;
        // having both is a conflict (the decorator *generates* `__init__`).
        if fieldwise_init
            && self
                .structs
                .get(name)
                .is_some_and(|i| i.methods.contains_key("__init__"))
        {
            return Err(TypeError::ConflictingConstructor(name.to_string()));
        }
        // Verify each declared conformance now that the method signatures exist.
        for tr in conforms {
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
                method_lowered_name(name, "__call__", selected)
            };
            self.structs
                .get_mut(name)
                .expect("callable struct remains registered")
                .callable_target = Some(target);
        }
        // Method bodies, each with `self` bound to this struct at its own type
        // parameters (so `self.field : Ty::Param` inside a generic struct).
        for (method_index, m) in methods.iter().enumerate() {
            self.check_method(self_ty, m, declaration.module.clone(), name, method_index)?;
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
        if BUILTIN_TRAITS.contains(&tr) {
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
            .map(|condition| self.compile_generic_constraint(condition))
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
                    };
                if !got_sigs.iter().any(|got| {
                    self.method_satisfies_requirement_under(
                        got,
                        &want,
                        conformance_assumption.as_ref(),
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
        method_satisfies_requirement(&normalized, required)
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
            "Movable" => self.is_movable(self_ty),
            "ImplicitlyDeletable" => true,
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
                        method.has_self
                            && method.self_convention == Some(ArgConvention::Mut)
                            && method.params == [Ty::String]
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
                        method.params.is_empty() && method.ret == Ty::UInt
                    })
                });
                initializes && updates && finishes
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
                let Ok(condition) = self.compile_generic_constraint(condition) else {
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
            CtValue::Str(_) => Some(Ty::String),
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
        if BUILTIN_TRAITS.contains(&tr) {
            return match tr {
                "AnyType" => true,
                "Copyable" => self.is_copyable(ty),
                "ImplicitlyCopyable" => self.is_implicitly_copyable(ty),
                "Movable" => self.is_movable(ty),
                "ImplicitlyDeletable" => self.is_implicitly_deletable(ty),
                "Hashable" => self.is_hashable(ty),
                "Writable" => match ty {
                    Ty::Struct(name, args) => self.struct_conformance_applies(name, args, tr),
                    Ty::Variant(alternatives) => alternatives
                        .iter()
                        .all(|alternative| self.conforms_to(alternative, tr)),
                    Ty::Param { bounds, .. } => bounds.iter().any(|bound| bound == tr),
                    Ty::Func { .. } | Ty::GenericFunc { .. } | Ty::Overload(_) => false,
                    _ => true,
                },
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
                "Intable" => is_numeric_like(ty) || *ty == Ty::Bool,
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
        match &expr.kind {
            ExprKind::Bool(value) => *value,
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
        let Ty::Struct(name, _) = ty else {
            return builtin_trait_operation(tr)
                .map(|operation| format!("missing required operation '{operation}'"));
        };
        let info = self.structs.get(name)?;
        let field_failure = |predicate: &dyn Fn(&Ty) -> bool| {
            info.fields
                .iter()
                .find(|(_, field_ty)| !predicate(field_ty))
                .map(|(field, field_ty)| {
                    format!("field '{field}' has type '{field_ty}', which is not {tr}")
                })
        };
        match tr {
            "Copyable" => field_failure(&|field_ty| self.is_copyable(field_ty)),
            "ImplicitlyCopyable" => {
                if info.methods.contains_key("__copyinit__") {
                    Some(
                        "defines '__copyinit__'; implicit copying requires fieldwise synthesis"
                            .to_string(),
                    )
                } else {
                    field_failure(&|field_ty| self.is_implicitly_copyable(field_ty))
                }
            }
            "ImplicitlyDeletable" => {
                field_failure(&|field_ty| self.is_implicitly_deletable(field_ty))
            }
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
        match ty {
            Ty::ComptimeList(element) => self.is_copyable(element),
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => {
                elements.iter().all(|element| self.is_copyable(element))
            }
            Ty::Variant(alternatives) => alternatives
                .iter()
                .all(|alternative| self.is_copyable(alternative)),
            Ty::Struct(name, _) => self
                .structs
                .get(name)
                .map(|s| {
                    s.conforms
                        .iter()
                        .any(|c| matches!(c.as_str(), "Copyable" | "ImplicitlyCopyable"))
                        || s.methods.contains_key("__copyinit__")
                })
                .unwrap_or(true),
            Ty::Param { bounds, .. } => bounds
                .iter()
                .any(|b| matches!(b.as_str(), "Copyable" | "ImplicitlyCopyable")),
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
        match ty {
            Ty::ComptimeList(element) => self.is_implicitly_copyable(element),
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => elements
                .iter()
                .all(|element| self.is_implicitly_copyable(element)),
            Ty::Variant(alternatives) => alternatives
                .iter()
                .all(|alternative| self.is_implicitly_copyable(alternative)),
            Ty::Struct(name, _) => self.structs.get(name).is_some_and(|s| {
                s.conforms.iter().any(|c| c == "ImplicitlyCopyable")
                    && self.struct_implicitly_copyable_conformance_ok(name)
            }),
            Ty::Param { bounds, .. } => bounds.iter().any(|b| b == "ImplicitlyCopyable"),
            _ => true,
        }
    }

    pub(super) fn is_movable(&self, _ty: &Ty) -> bool {
        // The current ownership model supports moving every initialized value.
        true
    }

    pub(super) fn is_implicitly_deletable(&self, ty: &Ty) -> bool {
        if self.has_assumed_conformance(ty, "ImplicitlyDeletable") {
            return true;
        }
        match ty {
            Ty::ComptimeList(element) => self.is_implicitly_deletable(element),
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => elements
                .iter()
                .all(|element| self.is_implicitly_deletable(element)),
            Ty::Variant(alternatives) => alternatives
                .iter()
                .all(|alternative| self.is_implicitly_deletable(alternative)),
            Ty::Struct(name, args) => self.structs.get(name).is_none_or(|info| {
                if info.conforms.iter().any(|tr| tr == "ImplicitlyDeletable") {
                    self.struct_conformance_applies(name, args, "ImplicitlyDeletable")
                } else {
                    true
                }
            }),
            Ty::Param { bounds, .. } => bounds.iter().any(|b| b == "ImplicitlyDeletable"),
            _ => true,
        }
    }

    pub(super) fn is_hashable(&self, ty: &Ty) -> bool {
        if self.has_assumed_conformance(ty, "Hashable") {
            return true;
        }
        match ty {
            Ty::Variant(alternatives) => alternatives
                .iter()
                .all(|alternative| self.is_hashable(alternative)),
            Ty::Struct(name, _) => self.structs.get(name).is_some_and(|s| {
                s.conforms.iter().any(|c| c == "Hashable") || s.methods.contains_key("__hash__")
            }),
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
        !info.methods.contains_key("__copyinit__")
            && info
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
        // A `^` transfer is `Expr::Transfer`, not a place, so it is naturally
        // exempt. A fresh temporary (a call result, a literal, an operator) is not a
        // place either — moving it is free.
        if is_place_expr(expr) {
            if !self.is_copyable(ty) {
                return Err(TypeError::NonCopyable {
                    ty: ty.to_string(),
                    context: context.to_string(),
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
        // The built-in `Hashable` trait contributes `__hash__(self) -> UInt`
        // (roadmap milestone 6). A user trait cannot shadow a built-in name, so this is
        // unambiguous.
        if method == "__hash__" && argc == 0 && bounds.iter().any(|b| b == "Hashable") {
            methods.push(MethodSig::intrinsic(vec![], Ty::UInt));
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
    ) -> Option<Vec<crate::ast::TypeParam>> {
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

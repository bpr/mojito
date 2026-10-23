//! Trait and struct declaration checking, trait conformance (nominal and
//! built-in), type-capability queries (Copyable/Movable/Hashable/…), and
//! trait-method/associated-member lookup. Extracted from `checker.rs`;
//! see `docs/symbol-map.md`.

use super::decorators::DecoratorSite;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::declaration::{Witness, WitnessTable, compiler_trait_declaration};
use mojito_symbol::symbol::{TRAIT_DISPATCH, qualified_method_symbol};
use mojito_types::conformance::{LIFECYCLE_TRAITS, StructTraitRule, struct_trait_rule};
use mojito_types::types::{TYPE_OF_ALL_TYPES, TyKind};

/// How a consuming position takes its value: an ownership **move** into new
/// storage (gated on `Movable`), or a **deinit** binding — consumption by a
/// destructor/named-destructor receiver or `deinit` parameter, which must stay
/// legal for a non-Movable (`Movable where False`) value.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ConsumeKind {
    Move,
    Deinit,
}

/// Whether a type conforms to `RegisterPassable`, as a result origin asks
/// it: a register-passed value has no memory a result could borrow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RegisterPassability {
    /// It conforms, under every binding of what it names.
    Proven,
    /// It might conform: a type parameter, an associated or abstract
    /// `Self` type, or a struct whose declared conformance is conditional.
    Possible,
    /// It does not conform.
    Excluded,
}

/// A trait's own defaulted `comptime` requirements, each as the member a
/// conformer that omits it declares: the requirement's annotation over the
/// default, as upstream synthesizes the conformer's witness. A parameterized
/// requirement's default keeps the requirement's parameters and `where`
/// clauses, so the synthesized member is applied as the conformer's own.
pub(super) fn comptime_defaults(members: &[TraitComptime]) -> Vec<StructComptime> {
    members
        .iter()
        .filter_map(|member| {
            Some(StructComptime {
                name: member.name.clone(),
                params: member.params.clone(),
                ty: Some(member.ty.clone()),
                where_clauses: member.where_clauses.clone(),
                value: member.default.clone()?,
            })
        })
        .collect()
}

impl Checker {
    /// A trait name is valid if it is a built-in, a user trait defined so
    /// far, or a same-module trait predeclared ahead of the struct shells.
    pub(super) fn check_trait_name(&self, name: &str) -> Result<(), TypeError> {
        if COMPILER_TRAITS.contains(&name)
            || name == TYPE_OF_ALL_TYPES
            || self.traits.contains_key(name)
            || self.predeclared_traits.contains(name)
        {
            Ok(())
        } else {
            Err(TypeError::UnknownTrait(name.to_string()))
        }
    }

    /// The canonical trait set `expr` denotes when it is a trait, or a `&`
    /// composition of traits, used as a value (`Copyable & Hashable`): each
    /// trait another one implies is dropped, as upstream folds a redundant
    /// composition. `None` when it names no trait.
    pub(super) fn trait_value(&self, expr: &Expr) -> Option<Result<Vec<String>, TypeError>> {
        let names = mojito_ast::ast::trait_conjunction_names(expr)?;
        let names_trait = |name: &str| {
            self.check_trait_name(name).is_ok()
                && self.lookup(name).is_none()
                && self.value_parameter_in_scope(name).is_none()
        };
        if !names.iter().any(|name| names_trait(name)) {
            return None;
        }
        Some(
            names
                .iter()
                .try_for_each(|name| self.check_trait_name(name))
                .map(|()| self.canonical_trait_set(&names)),
        )
    }

    /// The canonical trait set of a composition of `names`: each trait
    /// another one implies is dropped, the rest in [`trait_set`] order.
    ///
    /// [`trait_set`]: mojito_types::types::trait_set
    pub(super) fn canonical_trait_set<S: AsRef<str>>(&self, names: &[S]) -> Vec<String> {
        mojito_types::types::trait_set(
            names
                .iter()
                .map(AsRef::as_ref)
                .filter(|name| {
                    !names.iter().any(|other| {
                        other.as_ref() != *name && self.trait_implies(other.as_ref(), name)
                    })
                })
                .map(str::to_string),
        )
    }

    /// Whether the trait set `traits` converts to a trait metatype over
    /// `bound`: every trait of `bound` is implied by one of `traits`, as
    /// upstream upcasts `AnyTrait[Derived]` to `AnyTrait[Base]`.
    pub(super) fn traits_satisfy(&self, traits: &[String], bound: &[String]) -> bool {
        bound.iter().all(|required| {
            traits
                .iter()
                .any(|available| self.trait_implies(available, required))
        })
    }

    /// Whether a raw list of `from` converts to a raw list of `to`
    /// (`KGENParamList[Copyable]` to `KGENParamList[AnyType]`): the element
    /// is the same, or `to` is a trait every type of `from` conforms to.
    pub(super) fn param_list_converts(&self, from: &Ty, to: &Ty) -> bool {
        let (TyKind::ParamList(from), TyKind::ParamList(to)) = (from.kind(), to.kind()) else {
            return false;
        };
        from == to
            || matches!(to.kind(), TyKind::Trait(required)
                if mojito_types::types::trait_type_bound(from)
                    .is_some_and(|available| self.traits_satisfy(available, required)))
    }

    /// Upstream names a trait's members only through a conforming type or a
    /// bound, so `Trait.method(x)` is rejected rather than read as a value.
    pub(super) fn reject_direct_trait_member(&self, object: &Expr) -> Result<(), TypeError> {
        match &object.kind {
            ExprKind::Identifier(name)
                if self.traits.contains_key(name)
                    && !self.structs.contains_key(name)
                    && self.lookup(name).is_none() =>
            {
                Err(TypeError::DirectTraitMemberAccess)
            }
            _ => Ok(()),
        }
    }

    /// Register and check a `trait`: its method requirements (each typed with
    /// `Self` as the abstract conforming type, `TyKind::SelfType`).
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
            if COMPILER_TRAITS.contains(&parent.as_str()) && !self.traits.contains_key(parent) {
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
            let requirement = match self.ct_member_req_from_anno(&member.params, &member.ty)? {
                CtMemberReq::Value(result) if !member.params.is_empty() => {
                    self.generator_requirement(name, member, result)?
                }
                requirement => requirement,
            };
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
        // Requirement signatures resolve `Self` to the abstract `TyKind::SelfType`.
        let saved_self_ty = self.self_ty.replace(Ty::self_type());
        let saved_self_decls = std::mem::take(&mut self.self_decls);
        self.trait_self_comptime.push(ct_members.clone());
        let result = (|| {
            let mut sigs: HashMap<String, Vec<MethodSig>> = HashMap::new();
            let mut default_overloads = Vec::new();
            let mut requirements = Vec::new();
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
                self.check_decorators(
                    DecoratorSite::TraitMethod {
                        init: m.name == "__init__",
                    },
                    &m.decorators,
                )?;
                self.validate_origin_signature(&m.type_params, &m.params, m.self_origin.as_ref())?;
                if ct_members.contains_key(&m.name) {
                    return Err(TypeError::Redeclaration(m.name.clone()));
                }
                if m.positional_only.is_some() || m.keyword_only.is_some() {
                    return Err(TypeError::Unsupported(
                        "positional-only/keyword-only markers on trait methods".to_string(),
                    ));
                }
                // An overloaded requirement owns its binders under the
                // symbol its signature qualifies it to, so two overloads
                // whose slots agree still bind distinct parameters.
                let owner = mojito_symbol::symbol::lowered_method_name(
                    &format!("{}.{}", binder_owner(name), binder_owner(&m.name)),
                    &[],
                    mojito_symbol::symbol::MethodShape {
                        type_params: &m.type_params,
                        params: &m.params,
                        keyword_only: m.keyword_only,
                        has_self: true,
                        self_convention: m.self_convention,
                    },
                    &*self.overload_sets,
                    &|name| self.program_constants.get(name),
                );
                let mut decls = self.classify_params(&owner, &m.type_params)?;
                for condition in &m.where_clauses {
                    let constraint = self
                        .compile_where_clause(condition)
                        .map(|constraint| self.bind_constraint(&constraint, &decls))?;
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
                self.push_param_scope(&decls);
                let signature = (|| {
                    Ok::<_, TypeError>((
                        m.params
                            .iter()
                            .map(|param| {
                                param
                                    .default
                                    .as_ref()
                                    .map(|default| self.requirement_default(default, &decls))
                                    .transpose()
                            })
                            .collect::<Result<Vec<_>, _>>()?,
                        self.param_tys(&m.params)?,
                        match &m.ret {
                            Some(SourceType::Ref { referent, .. }) => {
                                self.ty_from_anno(referent)?
                            }
                            Some(t) => self.ty_from_anno(t)?,
                            None => Ty::none(),
                        },
                        self.declared_error(m.raises, m.raises_type.as_ref())?,
                    ))
                })();
                self.tparams.pop();
                let (defaults, all_types, ret, error) = signature?;
                self.check_result_origins(
                    m.ret.as_ref(),
                    &m.params,
                    &all_types,
                    Some((m.self_convention, &Ty::self_type())),
                )?;
                let collectors = Collectors::of(&m.params, m.keyword_only, &all_types);
                if let Some(index) = collectors.kw_variadic_index {
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
                    required: required_mask(&regular_params, collectors.keyword_only)?,
                    defaults: regular
                        .iter()
                        .map(|(index, _)| defaults[*index].clone())
                        .collect(),
                    variadic: collectors.variadic,
                    variadic_index: collectors.variadic_index,
                    variadic_convention: collectors.variadic_convention,
                    variadic_name: collectors.variadic_name,
                    kw_variadic: collectors.kw_variadic,
                    kw_variadic_index: collectors.kw_variadic_index,
                    positional_only: m.positional_only,
                    keyword_only: collectors.keyword_only,
                    conventions: regular.iter().map(|(_, param)| param.convention).collect(),
                    ret,
                    ret_mlir_index: super::type_resolution::spells_mlir_index(m.ret.as_ref()),
                    raises: error.as_ref().is_some_and(|ty| *ty != TyKind::Never),
                    error,
                    self_convention: m.self_convention,
                    ref_params: lower_ref_param_sigs(&m.type_params, &regular_params, 0)?,
                    ref_return,
                    view_return_interior: view_return_interior_tags(m.ret.as_ref()),
                    view_return: Vec::new(),
                    implicit: false,
                    parametric_origin_writes: Vec::new(),
                    origin_binders: vec![None; regular_params.len()],
                    synthesized_default: false,
                    nested_origins: NestedOrigins::AsDeclared,
                    overload: None,
                };
                let overloads = sigs.entry(m.name.clone()).or_default();
                if overloads.iter().any(|existing| {
                    same_method_shape(existing, &sig)
                        && (m.name != "__iter__" || existing.self_convention == sig.self_convention)
                }) {
                    return Err(TypeError::Redeclaration(m.name.clone()));
                }
                if m.default_body.is_some() {
                    default_overloads.push(overloads.len());
                }
                requirements.push((m.name.clone(), m.params.len(), overloads.len()));
                overloads.push(sig);
            }
            Ok((sigs, default_overloads, requirements))
        })();
        self.trait_self_comptime.pop();
        self.self_ty = saved_self_ty;
        self.self_decls = saved_self_decls;
        let shapes =
            super::trait_defaults::default_shapes(name, methods, &*self.overload_sets, &|name| {
                self.program_constants.get(name)
            });
        let (methods, default_overloads, requirements) = result?;
        let defaults = shapes
            .into_iter()
            .zip(default_overloads)
            .map(|(shape, overload)| shape.at_overload(overload))
            .collect();
        self.traits.insert(
            name.to_string(),
            TraitInfo {
                refines: refines.to_vec(),
                methods,
                comptime_members: ct_members,
                comptime_constraints: ct_constraints,
                comptime_defaults: comptime_defaults(comptime_members),
                defaults,
                requirements,
            },
        );
        Ok(())
    }

    /// Register a struct and check its method bodies. A generic struct's type
    /// parameters are validated and kept in scope (as `Self.T`) for its fields
    /// and methods; field/method types referring to them become `TyKind::Param`.
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
        let decls = self.classify_params(name, type_params)?;
        self.generic_parameters.borrow_mut().insert(
            mojito_checked::checked::GenericSite::Struct {
                module: declaration.module.clone(),
                declaration: name.to_string(),
            },
            decls.clone(),
        );

        let explicit_destroy_message = self
            .check_decorators(
                DecoratorSite::Struct {
                    name,
                    conditional_deinitable: declaration
                        .conformance_conditions
                        .iter()
                        .any(|(trait_name, _)| trait_name == "Deinitable"),
                },
                declaration.decorators,
            )?
            .explicit_destroy;
        let explicit_destructors = declaration
            .methods
            .iter()
            .filter(|method| {
                method.name != "__deinit__" && method.self_convention == Some(ArgConvention::Deinit)
            })
            .map(|method| (method.name.clone(), method.raises))
            .chain(self.inherited_destructors(declaration))
            .collect::<HashMap<_, _>>();
        self.structs.insert(
            name.to_string(),
            StructInfo {
                decls,
                source_params: type_params.to_vec(),
                conforms: declaration.conforms.to_vec(),
                callable_conformance: None,
                callable_target: None,
                witness_tables: Vec::new(),
                conformance_conditions: declaration
                    .conformance_conditions
                    .iter()
                    .cloned()
                    .collect(),
                fields: Vec::new(),
                declared_field_names: declaration
                    .fields
                    .iter()
                    .map(|field| field.name.clone())
                    .collect(),
                field_origin_arguments: HashMap::new(),
                associated: HashMap::new(),
                associated_value_types: HashMap::new(),
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
    /// are in scope as `Self.T` / `Self.n` (type parameters as `TyKind::Param`,
    /// value parameters as symbolic `CtValue::Expr`) and bare `Self` is the
    /// struct type. Returns the `Self` type and the saved outer scope for
    /// `exit_struct_scope`.
    pub(super) fn enter_struct_scope(
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
        let self_ty = Ty::new(TyKind::Struct(name.to_string(), info.self_arguments()));
        let saved = SavedStructScope {
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

    pub(super) fn exit_struct_scope(&mut self, saved: SavedStructScope) {
        self.self_decls = saved.self_decls;
        self.enclosing_type_params = saved.type_params;
        self.self_ty = saved.self_ty;
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
                let constraint = constraint.bind(&[&info.decls]);
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
            .is_some_and(|ty| !matches!(ty.kind(), TyKind::Func(_)))
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
        // Associated members resolve BEFORE fields, each installed into the
        // registered shell as it resolves: a later member reads an earlier
        // one through `Self`, and a field's type may apply the struct's own
        // comptime alias (`var iter: Self.dict_entry_iter`). Member bodies
        // reference only `Self` parameters and registered struct shells, never
        // fields, so this ordering is well-founded.
        let (
            associated_values,
            associated_value_types,
            associated_constraints,
            parameterized_associated,
        ) = self.check_struct_associated(
            declaration.associated,
            declaration.conforms,
            declaration.conformance_conditions,
        )?;
        if let Some(info) = self.structs.get_mut(name) {
            info.associated.clone_from(&associated_values);
            info.associated_value_types = associated_value_types;
            info.associated_constraints
                .clone_from(&associated_constraints);
            info.parameterized_associated
                .clone_from(&parameterized_associated);
        }
        // Field types resolve with every module struct shell registered, so a
        // field may reference a struct declared later in the module (by-value
        // self-containment is rejected separately at completion); duplicate
        // field names are a redeclaration.
        let mut field_tys: Vec<(String, Ty)> = Vec::new();
        let mut field_origin_arguments = HashMap::new();
        let self_display = self.self_ty.as_ref().map(std::string::ToString::to_string);
        // A generated specialization's annotations rebind checked types, so
        // a nested origin slot they leave unbound is not a concreteness error.
        self.generated_declaration
            .set(declaration.name.contains('$'));
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
            // A field typed by a bare struct type parameter needs a
            // `Deinitable`-proving bound, unless the struct itself declares
            // `Deinitable` (conditionally, as `Optional`/`List` do), which
            // discharges the field's destructibility to that declaration.
            // A `Tuple` spreading a pack is `Deinitable` only where every
            // element is, so the pack's bound must prove it too.
            if !declaration.name.contains('$')
                && !declaration.conforms.iter().any(|tr| tr == "Deinitable")
            {
                let undestroyable = match ty.kind() {
                    TyKind::Param { binder, bounds, .. }
                        if !self.bounds_prove_deinitable(bounds) =>
                    {
                        Some(binder.name.trim_start_matches('*').to_string())
                    }
                    TyKind::Struct(name, arguments)
                        if mojito_types::types::is_nominal_instance(
                            name,
                            mojito_types::types::TUPLE_TYPE_NAME,
                        ) && matches!(
                            (mojito_types::types::pack_spread_argument(arguments)).map(Ty::kind),
                            Some(TyKind::Param { bounds, .. })
                                if !self.bounds_prove_deinitable(bounds)
                        ) =>
                    {
                        Some(ty.to_string())
                    }
                    _ => None,
                };
                if let Some(ty) = undestroyable {
                    return Err(TypeError::FieldNotDeinitable {
                        field: f.name.clone(),
                        ty,
                    });
                }
            }
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
        self.generated_declaration.set(name.contains('$'));
        for (method_index, m) in declaration.methods.iter().enumerate() {
            let method_name = lifecycle_method_name(m);
            let method_decls =
                self.classify_params(&self.method_binder_owner(name, m), &m.type_params)?;
            self.generic_parameters.borrow_mut().insert(
                mojito_checked::checked::GenericSite::Method {
                    module: declaration.module.clone(),
                    declaration: name.to_string(),
                    method: method_index,
                },
                method_decls.clone(),
            );
            self.push_param_scope(&method_decls);
            let saved_method_type_params = self.enclosing_type_params.clone();
            let saved_struct_count = self
                .enclosing_struct_type_params
                .replace(saved_method_type_params.len());
            self.enclosing_type_params.extend(m.type_params.clone());
            // The method's own `where` clause refines its signature exactly as
            // it refines its body: a conditionally available method may name
            // types its receiver's bare bounds do not admit.
            let signature = match self.method_where_assumptions(m, &method_decls) {
                Ok(assumptions) => {
                    self.assume_propositions(Vec::new());
                    self.assumed_conformances.borrow_mut().push(assumptions);
                    let signature = (|| {
                        let all_types = self.param_tys(&m.params)?;
                        let sig = self.method_sig(m, method_decls, &all_types)?;
                        if !name.contains('$') && !m.name.contains('$') {
                            let receiver = self.self_ty.as_ref().filter(|_| m.has_self);
                            self.check_result_origins(
                                m.ret.as_ref(),
                                &m.params,
                                &all_types,
                                receiver.map(|ty| (m.self_convention, ty)),
                            )?;
                        }
                        Ok::<_, TypeError>((all_types, sig))
                    })();
                    self.assumed_conformances.borrow_mut().pop();
                    signature
                }
                Err(error) => Err(error),
            };
            self.enclosing_type_params = saved_method_type_params;
            self.enclosing_struct_type_params.set(saved_struct_count);
            self.tparams.pop();
            let (all_types, mut sig) = signature?;
            sig.overload = self
                .method_binder_owners
                .call_qualifier(name, m)
                .map(str::to_string);
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
                    error: sig.raises.then(|| sig.error.clone()).flatten(),
                    returns_reference: sig.ref_return.is_some(),
                    param_writes: mojito_checked::checked::DeclarationEffect::param_writes(
                        &sig.conventions,
                        &sig.ref_params,
                    ),
                    availability: sig.availability.clone(),
                },
            );
            let info = self.structs.get_mut(name).ok_or_else(|| {
                TypeError::InvariantViolation(format!("struct '{name}' was not registered"))
            })?;
            let overloads = info.methods.entry(method_name.to_string()).or_default();
            if overloads.iter().any(|existing| {
                same_method_shape(existing, &sig)
                    && existing.has_self == sig.has_self
                    && (!mojito_symbol::symbol::receiver_overloaded_method(method_name)
                        || existing.self_convention == sig.self_convention)
            }) {
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
        self.register_default_wrappers(declaration)
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
        // Verify each declared conformance now that the method signatures
        // exist, recording each one's witness table.
        let mut witness_tables = Vec::new();
        for tr in declaration.conforms {
            witness_tables.extend(self.verify_conformance(name, tr, self_ty)?);
        }
        witness_tables.sort_by(|left: &WitnessTable, right| left.trait_name.cmp(&right.trait_name));
        witness_tables.dedup_by(|left, right| left.trait_name == right.trait_name);
        if let Some(info) = self.structs.get_mut(name) {
            info.witness_tables = witness_tables;
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
        // parameters (so `self.field : TyKind::Param` inside a generic struct).
        let mut overload_indices = HashMap::<String, usize>::new();
        for (method_index, m) in methods.iter().enumerate() {
            let method_name = lifecycle_method_name(m).to_string();
            let overload_index = *overload_indices.entry(method_name.clone()).or_default();
            *overload_indices
                .get_mut(&method_name)
                .expect("inserted above") += 1;
            self.check_method(
                self_ty,
                m,
                declaration.module.clone().as_ref(),
                name,
                method_index,
                overload_index,
            )?;
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
            let TyKind::Struct(current_name, args) = current.kind() else {
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
            match ty.kind() {
                // A handle or pointer breaks the by-value containment chain.
                TyKind::Ref(_) | TyKind::Pointer { .. } => Ok(()),
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
    /// every method required by trait `tr`, with a matching signature.
    ///
    /// Answers the conformance's witness table: the member satisfying each
    /// method requirement, keyed by the requirement's dispatch symbol. A
    /// trait with no method requirements has no table. A few built-in marker
    /// traits have real lifecycle semantics; other built-ins remain shallow
    /// recognized bounds until their corresponding feature grows.
    pub(super) fn verify_conformance(
        &self,
        name: &str,
        tr: &str,
        self_ty: &Ty,
    ) -> Result<Option<WitnessTable>, TypeError> {
        // Validate the declaration shape even for builtin marker traits.
        // Truth is evaluated at each concrete use, but a malformed
        // `(condition, message)` tuple is always a declaration error. The
        // compiled condition is the premise a conditional builtin
        // conformance (`Copyable where conforms_to(K, Copyable)` over a
        // `var key: Self.K` field) verifies under.
        let declared_condition = self
            .structs
            .get(name)
            .and_then(|info| Some((info, info.conformance_conditions.get(tr)?)))
            .map(|(info, condition)| self.compile_condition(&info.decls, condition))
            .transpose()?;
        // The focused checker can recognize protocol bounds without linking the
        // implicit prelude, but a registered nominal trait is authoritative.
        // In production `Iterator`/`Iterable` are ordinary stdlib traits; the
        // builtin compatibility spelling must not bypass their requirements.
        if COMPILER_TRAITS.contains(&tr) && !self.traits.contains_key(tr) {
            self.verify_builtin_conformance(name, tr, self_ty, declared_condition.as_ref())?;
            return Ok(self.compiler_trait_witness_table(name, tr, self_ty));
        }
        let Some(trait_info) = self.traits.get(tr) else {
            return Ok(None);
        };
        let struct_info = self.structs.get(name).ok_or_else(|| {
            TypeError::InvariantViolation(format!(
                "struct '{name}' was not registered before conformance checking"
            ))
        })?;
        let conformance_assumption = struct_info
            .conformance_conditions
            .get(tr)
            .map(|condition| self.compile_condition(&struct_info.decls, condition))
            .transpose()?;
        let mut witnesses = Vec::new();
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
            for req_sig in req_sigs {
                let want = self.requirement_at(req_sig, self_ty);
                let got = got_sigs
                    .iter()
                    .find(|got| {
                        self.method_satisfies_requirement_under(
                            got,
                            &want,
                            conformance_assumption.as_ref(),
                            mname == "__next__",
                        )
                    })
                    .ok_or_else(|| TypeError::TraitMethodMismatch {
                        struct_name: name.to_string(),
                        trait_name: tr.to_string(),
                        method: mname.clone(),
                    })?;
                witnesses.push(Witness {
                    requirement: method_lowered_name(TRAIT_DISPATCH, mname, req_sig, None),
                    member: qualified_method_symbol(name, mname, got.overload.as_deref()),
                });
            }
        }
        witnesses.sort_by(|left, right| left.requirement.cmp(&right.requirement));
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
                // A parameterized value witnesses a generator requirement
                // when its signature and result type match it.
                if let (
                    CtMemberReq::Generator { params, result },
                    MemberTemplate::Value { decls, body },
                ) = (req, &parameterized.template)
                {
                    if !same_generator_signature(params, decls)
                        || !generator_result_satisfies(body, result)
                    {
                        return Err(TypeError::TraitComptimeMemberMismatch {
                            struct_name: name.to_string(),
                            trait_name: tr.to_string(),
                            member: member.clone(),
                        });
                    }
                    continue;
                }
                let (CtMemberReq::Type { bounds, params }, MemberTemplate::Type { .. }) =
                    (req, &parameterized.template)
                else {
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
                        constraints::AssocParamKind::Type => Some(TyArg::Ty(
                            TyKind::Param {
                                binder: synthetic_binder(&format!("$placeholder:{}", param.name)),
                                bounds: param.bounds.clone(),
                                callable_bound: None,
                            }
                            .into(),
                        )),
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
        Ok((!witnesses.is_empty()).then(|| WitnessTable {
            trait_name: tr.to_string(),
            witnesses,
        }))
    }

    /// The witness table of struct `name`'s conformance to the
    /// compiler-declared trait `tr` (`Defaultable`, `Comparable`).
    ///
    /// It records, its parents' requirements included, each requirement the
    /// struct declares a method for whose parameters are the requirement's,
    /// `Self` the struct: the nullary `__init__` overload for
    /// `Defaultable`, a comparison taking the struct for the others.
    fn compiler_trait_witness_table(
        &self,
        name: &str,
        tr: &str,
        self_ty: &Ty,
    ) -> Option<WitnessTable> {
        let methods = &self.structs.get(name)?.methods;
        let mut pending = vec![compiler_trait_declaration(tr)?];
        let mut witnesses = Vec::new();
        while let Some(declaration) = pending.pop() {
            pending.extend(
                declaration
                    .refines
                    .iter()
                    .filter_map(|parent| compiler_trait_declaration(parent)),
            );
            witnesses.extend(declaration.requirements.iter().filter_map(|requirement| {
                let got = methods.get(&requirement.name)?.iter().find(|got| {
                    got.has_self
                        && got.variadic.is_none()
                        && got.kw_variadic.is_none()
                        && got.params.len() == requirement.param_types.len()
                        && got
                            .params
                            .iter()
                            .zip(&requirement.param_types)
                            .all(|(got, want)| *got == substitute_self(want, self_ty))
                })?;
                Some(Witness {
                    requirement: requirement.dispatch.clone(),
                    member: qualified_method_symbol(
                        name,
                        &requirement.name,
                        got.overload.as_deref(),
                    ),
                })
            }));
        }
        witnesses.sort_by(|left, right| left.requirement.cmp(&right.requirement));
        (!witnesses.is_empty()).then(|| WitnessTable {
            trait_name: tr.to_string(),
            witnesses,
        })
    }

    pub(super) fn method_satisfies_requirement_under(
        &self,
        got: &MethodSig,
        required: &MethodSig,
        conformance_assumption: Option<&GenericConstraint>,
        allow_copyable_iterator_reference: bool,
    ) -> bool {
        // A witness may spell its own binders as it likes, so a clause names
        // them by position on both sides.
        let required_availability = canonical_availability(required);
        let availability_is_covered = canonical_availability(got)
            .iter()
            .zip(&got.availability)
            .all(|(canonical, constraint)| {
                required_availability
                    .iter()
                    .any(|premise| generic_constraint_implies(premise, canonical))
                    || conformance_assumption
                        .is_some_and(|premise| generic_constraint_implies(premise, constraint))
            });
        if !availability_is_covered {
            return false;
        }
        // Availability was proved above. Normalize it to the requirement before
        // comparing the remainder of the callable contract.
        let mut normalized = got.clone();
        normalized.availability.clone_from(&required.availability);
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

    /// A trait requirement as struct `Self` type `self_ty` must satisfy it:
    /// the requirement's `Self` becomes the struct's type, and receiver
    /// conventions are part of the contract.
    pub(super) fn requirement_at(&self, req_sig: &MethodSig, self_ty: &Ty) -> MethodSig {
        MethodSig {
            decls: req_sig.decls.clone(),
            availability: req_sig.availability.clone(),
            has_self: true,
            params: req_sig
                .params
                .iter()
                .map(|t| self.requirement_ty_at(t, self_ty))
                .collect(),
            names: req_sig.names.clone(),
            required: req_sig.required.clone(),
            defaults: req_sig.defaults.clone(),
            variadic: req_sig
                .variadic
                .as_ref()
                .map(|ty| self.requirement_ty_at(ty, self_ty)),
            variadic_index: req_sig.variadic_index,
            variadic_convention: req_sig.variadic_convention,
            variadic_name: req_sig.variadic_name.clone(),
            kw_variadic: req_sig
                .kw_variadic
                .as_ref()
                .map(|ty| self.requirement_ty_at(ty, self_ty)),
            kw_variadic_index: req_sig.kw_variadic_index,
            positional_only: req_sig.positional_only,
            keyword_only: req_sig.keyword_only,
            conventions: req_sig.conventions.clone(),
            ret: self.requirement_ty_at(&req_sig.ret, self_ty),
            ret_mlir_index: req_sig.ret_mlir_index,
            raises: req_sig.raises,
            error: req_sig
                .error
                .as_ref()
                .map(|error| self.requirement_ty_at(error, self_ty)),
            self_convention: req_sig.self_convention,
            ref_params: req_sig.ref_params.clone(),
            ref_return: req_sig.ref_return.clone(),
            view_return_interior: req_sig.view_return_interior.clone(),
            view_return: req_sig.view_return.clone(),
            implicit: req_sig.implicit,
            parametric_origin_writes: req_sig.parametric_origin_writes.clone(),
            origin_binders: req_sig.origin_binders.clone(),
            synthesized_default: false,
            nested_origins: req_sig.nested_origins,
            overload: None,
        }
    }

    /// One type of a requirement's signature at the conformer `self_ty`:
    /// `Self` and its value members replaced, associated types resolved.
    fn requirement_ty_at(&self, ty: &Ty, self_ty: &Ty) -> Ty {
        self.resolve_assoc_ty(&self.substitute_self_members(ty, self_ty))
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
        match ty.kind() {
            TyKind::ComptimeList(element) => self.is_copyable_under_assumption(element, assumption),
            TyKind::Tuple(elements) | TyKind::RuntimePack(elements) => elements
                .iter()
                .all(|element| self.is_copyable_under_assumption(element, assumption)),
            TyKind::Variant(alternatives) => alternatives
                .iter()
                .all(|alternative| self.is_copyable_under_assumption(alternative, assumption)),
            TyKind::Param { binder, bounds, .. } => {
                bounds.iter().any(|bound| {
                    matches!(bound.as_str(), "Copyable" | "ImplicitlyCopyable")
                        || self.trait_refines(bound, "Copyable")
                }) || assumption
                    .is_some_and(|known| binder_conformance_assumed(binder, "Copyable", known))
            }
            TyKind::Assoc { .. } => self.assoc_member_bound_proves(ty, "Copyable"),
            TyKind::Struct(name, arguments) => {
                let Some(info) = self.structs.get(name) else {
                    // A compatibility spelling may produce an opaque nominal
                    // type without registering its declaration. That is not a
                    // Copyable proof strong enough to change a method ABI.
                    return false;
                };
                let arguments = positional_pack_arguments(&info.decls, arguments);
                let environment = ConstraintEnvironment::declared(&info.decls, &arguments);
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
                    let Ok(condition) = self.compile_condition(&info.decls, condition) else {
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
            TyKind::VariadicPack(element) => self.is_copyable_under_assumption(element, assumption),
            // These types still depend on substitution or trait context. The
            // ordinary capability helper defaults unknown concrete spellings to
            // copyable for compatibility, but that is not a proof strong enough
            // to change an abstract method ABI.
            TyKind::Dependent(_) | TyKind::SelfType | TyKind::Infer => false,
            _ => self.is_copyable(ty),
        }
    }

    pub(super) fn verify_builtin_conformance(
        &self,
        name: &str,
        tr: &str,
        self_ty: &Ty,
        assumption: Option<&GenericConstraint>,
    ) -> Result<(), TypeError> {
        let ok = match tr {
            "Copyable" => self.struct_copyable_conformance_ok(name, assumption),
            "ImplicitlyCopyable" => {
                self.struct_implicitly_copyable_conformance_ok(name, assumption)
            }
            // A declared narrowing conformance (`Movable where False`) must
            // verify at declaration like `Deinitable where False`;
            // effectiveness is enforced at the transfer/consuming use sites.
            "Movable" => true,
            "Deinitable" => true,
            "Indexer" => self.structs.get(name).is_some_and(|info| {
                info.methods
                    .get("__mlir_index__")
                    .is_some_and(|methods| methods.iter().any(is_indexer_requirement))
            }),
            "Hasher" => self.structs.get(name).is_some_and(|info| {
                let initializes = info.methods.get("__init__").is_some_and(|methods| {
                    methods.iter().any(|method| method.params.is_empty())
                });
                let updates = info.methods.get("update").is_some_and(|methods| {
                    methods.iter().any(|method| {
                        method.self_convention == Some(ArgConvention::Mut)
                            && method.params.len() == 1
                            && method.ret == TyKind::None
                    })
                });
                let finishes = info.methods.get("finish").is_some_and(|methods| {
                    methods.iter().any(|method| {
                        method.params.is_empty()
                            && method.self_convention == Some(ArgConvention::Var)
                            && method.ret == canonical_simd_ty(mojito_ast::ast::Dtype::UInt64, 1)
                    })
                });
                // `_update_with_simd(mut self, value: SIMD[_, _])`: the vector
                // parameter is open in both slots, over the inferred binders
                // the elaborator desugars the wildcard spelling to.
                let simd_updates = info.methods.get("_update_with_simd").is_some_and(|methods| {
                    methods.iter().any(|method| {
                        method.self_convention == Some(ArgConvention::Mut)
                            && matches!(
                                method.params.as_slice(),
                                [param] if matches!(
                                    param.kind(),
                                    TyKind::Simd {
                                        dtype: SimdDtype::Expr(_),
                                        width: SimdWidth::Expr(_),
                                    }
                                )
                            )
                            && method.ret == TyKind::None
                    })
                });
                let byte_updates = info.methods.get("_update_with_bytes").is_some_and(|methods| {
                    methods.iter().any(|method| {
                        method.self_convention == Some(ArgConvention::Mut)
                            && method.params.len() == 1
                            && method.ret == TyKind::None
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
                            && matches!((method.params[0]).kind(), TyKind::Param { bounds, .. } if bounds.iter().any(|bound| bound == "Hasher"))
                            && method.ret == TyKind::None
                    })
                })
            }),
            // A struct without a `Writer`-fed `write_to` writes through the
            // reflective `Name(field=...)` default; any other `write_to` is an
            // ordinary overload beside it, as in current Mojo.
            "Writable" => self.structs.contains_key(name),
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
            // A literal witnesses the type it materializes as (`comptime
            // NAME = "a"` for `comptime NAME: String`).
            CtMemberReq::Value(expected) => {
                self.ct_value_ty(value, self_ty)
                    .is_some_and(|actual| coerces(&actual, expected))
                    || value.clone().materialize_as(expected).is_some()
                    || self.open_type_list_satisfies(value, expected)
            }
            CtMemberReq::Type { bounds, .. } => {
                let CtValue::Type(ty) = value else {
                    return false;
                };
                bounds.iter().all(|bound| {
                    self.conforms_to_under_assumption(ty, bound, conformance_assumption)
                })
            }
            // An unparameterized member is no generator.
            CtMemberReq::Generator { .. } => false,
        }
    }

    /// Whether `value`, a raw list of types over open parameters
    /// (`TypeList.of[Trait=AnyType, NoneType, Self.T].values`), witnesses
    /// the raw list `expected`: its element meta-type keeps no trait, so it
    /// does when every type conforms to the element's
    /// (`KGENParamList[AnyType]`).
    fn open_type_list_satisfies(&self, value: &CtValue, expected: &Ty) -> bool {
        let CtValue::Expr(list) = value else {
            return false;
        };
        let TyKind::ParamList(element) = expected.kind() else {
            return false;
        };
        list.meta() == &mojito_types::param_expr::MetaTy::type_list()
            && mojito_types::types::trait_type_bound(element)
                .is_some_and(|required| self.traits_satisfy(&["AnyType".to_string()], required))
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
        if let TyKind::Param { binder, .. } = ty.kind() {
            let needed = GenericConstraint::Conforms {
                param: binder.clone(),
                trait_name: required.to_string(),
            };
            return assumption.is_some_and(|known| generic_constraint_implies(known, &needed));
        }
        let TyKind::Struct(name, args) = ty.kind() else {
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
                let Ok(condition) = self.compile_condition(&info.decls, condition) else {
                    return false;
                };
                let args = positional_pack_arguments(&info.decls, args);
                let environment = ConstraintEnvironment::declared(&info.decls, &args);
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
        environment: &ConstraintEnvironment<'_>,
        assumption: Option<&GenericConstraint>,
        visiting: &mut HashSet<(String, String)>,
    ) -> bool {
        use GenericConstraint::{And, Conforms, ConformsPack, Not, Or, WithMessage};
        match constraint {
            WithMessage(condition, _) => self.eval_constraint_under_assumption(
                condition,
                environment,
                assumption,
                visiting,
            ),
            Conforms { param, trait_name } => environment
                .get(param)
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
                .get(param)
                .is_some_and(|argument| match argument {
                    TyArg::Val(CtValue::Tuple(values)) => values.iter().all(|value| {
                        matches!(value, CtValue::Type(ty) if self.conforms_to_under_assumption_inner(
                            ty,
                            trait_name,
                            assumption,
                            visiting,
                        ))
                    }),
                    // A pack bound to another declaration's pack that is
                    // still a parameter (`Tuple[*Self.Ts]`): its elements
                    // are that pack's, whose bound or the premise decides.
                    TyArg::Ty(pack)
                        if let TyKind::Param { binder, .. } = pack.kind()
                            && binder.name.starts_with('*') =>
                    {
                        self.conforms_to(pack, trait_name)
                            || assumption.is_some_and(|known| {
                                binder_conformance_assumed(binder, trait_name, known)
                            })
                    }
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
            CtValue::Int(_) | CtValue::Deferred(_) | CtValue::Marker(_) => Some(Ty::int()),
            CtValue::Expr(expr) => expr
                .meta()
                .value_ty()
                .or_else(|| self.type_pack_list_ty(expr)),
            // A closed raw parameter list holds its elements as a tuple.
            CtValue::Tuple(values) if let TyKind::ParamList(element) = self_ty.kind() => {
                let first = values.first().map_or_else(
                    || Some(element.clone()),
                    |value| self.ct_value_ty(value, element),
                )?;
                values
                    .iter()
                    .all(|value| self.ct_value_ty(value, element).as_ref() == Some(&first))
                    .then(|| TyKind::ParamList(first).into())
            }
            CtValue::UInt(_) => Some(Ty::uint()),
            CtValue::Float(_) => Some(Ty::float64()),
            CtValue::IntLiteral(_) => Some(Ty::int_literal()),
            CtValue::FloatLiteral(_) => Some(Ty::float_literal()),
            CtValue::Bool(_) => Some(Ty::bool()),
            CtValue::Str(_) => Some(Ty::string_literal()),
            CtValue::Dtype(_) => Some(Ty::dtype()),
            CtValue::Trait(traits) => Some(TyKind::AnyTrait(traits.clone()).into()),
            CtValue::KgenString(_) => Some(TyKind::KgenString.into()),
            CtValue::Simd { dtype, lanes } => Some(mojito_types::types::canonical_simd_ty(
                *dtype,
                lanes.len() as i64,
            )),
            CtValue::Struct { name, .. } => Some(TyKind::Struct(name.clone(), Vec::new()).into()),
            CtValue::Tuple(values) => values
                .iter()
                .map(|v| self.ct_value_ty(v, self_ty))
                .collect::<Option<Vec<_>>>()
                .map(|elements| {
                    if matches!(self_ty.kind(), TyKind::Tuple(_) | TyKind::RuntimePack(_)) {
                        TyKind::Tuple(elements).into()
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
            // A display types like the checker's own inference of it: the
            // first entry's types, every other entry coercing, under the
            // default hasher; an explicit spelling is the type itself.
            CtValue::Dict { spelling, entries } => {
                if let Some(ty) = spelling {
                    Some((*ty).clone())
                } else {
                    let (first_key, first_value) = entries.first()?;
                    let key = self.ct_value_ty(first_key, self_ty)?;
                    let value = self.ct_value_ty(first_value, self_ty)?;
                    entries
                        .iter()
                        .skip(1)
                        .all(|(k, v)| {
                            self.ct_value_ty(k, self_ty)
                                .is_some_and(|ty| coerces(&ty, &key))
                                && self
                                    .ct_value_ty(v, self_ty)
                                    .is_some_and(|ty| coerces(&ty, &value))
                        })
                        .then(|| self.nominal_dict(key, value))
                }
            }
            CtValue::Set { spelling, elements } => {
                if let Some(ty) = spelling {
                    Some((*ty).clone())
                } else {
                    let element = self.ct_value_ty(elements.first()?, self_ty)?;
                    elements
                        .iter()
                        .skip(1)
                        .all(|v| {
                            self.ct_value_ty(v, self_ty)
                                .is_some_and(|ty| coerces(&ty, &element))
                        })
                        .then(|| self.nominal_set(element))
                }
            }
            CtValue::Pointer { .. }
            | CtValue::StoreToMem(_)
            | CtValue::Type(_)
            | CtValue::Reflected(_)
            | CtValue::RemovedArg => {
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
        if let Some(element) = self.opaque_element(ty) {
            return self.conforms_to(&element, tr);
        }
        if self.has_assumed_conformance(ty, tr) {
            return true;
        }
        // Every type is a `!kgen.type`, while a binder of it implies no
        // trait, not even `AnyType`.
        if tr == TYPE_OF_ALL_TYPES {
            return true;
        }
        if let TyKind::Param { bounds, .. } = ty.kind() {
            if bounds.iter().any(|bound| bound == tr) {
                return true;
            }
            if matches!(bounds.as_slice(), [only] if only == TYPE_OF_ALL_TYPES) {
                return false;
            }
        }
        if COMPILER_TRAITS.contains(&tr) && !self.traits.contains_key(tr) {
            // A hashed leaf is recorded as it is proved, so `is_hashable`
            // answers for its own.
            if tr != "Hashable"
                && let Some(conforms) =
                    mojito_types::conformance::leaf_conforms(ty, tr, &mut |element, tr| {
                        self.conforms_to(element, tr)
                    })
            {
                return conforms;
            }
            return match tr {
                "AnyType" => true,
                "Copyable" => self.is_copyable(ty),
                "ImplicitlyCopyable" => self.is_implicitly_copyable(ty),
                "Movable" => self.is_movable(ty),
                "Deinitable" => self.is_deinitable(ty),
                "Hashable" => self.is_hashable(ty),
                "Writable" => match ty.kind() {
                    // The intrinsic slice descriptors write as
                    // `Slice(start, end, step)` (upstream's `Writable`).
                    TyKind::Struct(name, args)
                        if args.is_empty()
                            && matches!(
                                name.as_str(),
                                "Slice" | "ContiguousSlice" | "StridedSlice"
                            ) =>
                    {
                        true
                    }
                    TyKind::Struct(name, args) => self.struct_conformance_applies(name, args, tr),
                    TyKind::Param { bounds, .. } => bounds.iter().any(|bound| bound == tr),
                    TyKind::Func(_) | TyKind::GenericFunc(_) | TyKind::Overload(_) => false,
                    _ => true,
                },
                "Hasher" => match ty.kind() {
                    TyKind::Struct(name, args) => self.struct_conformance_applies(name, args, tr),
                    TyKind::Param { bounds, .. } => bounds.iter().any(|bound| bound == tr),
                    _ => false,
                },
                // Default construction is a declared conformance on structs
                // (current `Tuple`/`Array` default-construct their elements
                // through it); the scalar built-ins and `None` answer
                // structurally.
                "Defaultable" => match ty.kind() {
                    TyKind::Int
                    | TyKind::IntLiteral
                    | TyKind::UInt
                    | TyKind::Float64
                    | TyKind::FloatLiteral
                    | TyKind::Bool
                    | TyKind::StringLiteral
                    | TyKind::None
                    | TyKind::Simd { .. } => true,
                    TyKind::Tuple(elements) | TyKind::RuntimePack(elements) => {
                        elements.iter().all(|element| self.conforms_to(element, tr))
                    }
                    TyKind::Struct(name, args) => self.struct_conformance_applies(name, args, tr),
                    TyKind::Param { bounds, .. } => bounds
                        .iter()
                        .any(|bound| bound == tr || self.trait_refines(bound, tr)),
                    _ => false,
                },
                "Indexer" => match ty.kind() {
                    TyKind::Int | TyKind::IntLiteral => true,
                    TyKind::Struct(name, args) => self.struct_conformance_applies(name, args, tr),
                    TyKind::Param { bounds, .. } => bounds.iter().any(|bound| bound == tr),
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
                // A struct declaring Intable (with its `__int__`) or
                // Floatable (with its `__float__`) conforms like the numeric
                // scalars — integer-Scalar construction accepts any Intable
                // value.
                "Intable" => {
                    is_numeric_like(ty)
                        || *ty == TyKind::Bool
                        || matches!(ty.kind(), TyKind::Struct(name, args)
                            if self.struct_conformance_applies(name, args, tr))
                }
                "Floatable" => {
                    is_numeric_like(ty)
                        || matches!(ty.kind(), TyKind::Struct(name, args)
                            if self.struct_conformance_applies(name, args, tr))
                }
                "RegisterPassable" => self.register_passability(ty) == RegisterPassability::Proven,
                // Layout/backend markers and future operation traits stay shallow.
                _ => true,
            };
        }
        match ty.kind() {
            TyKind::Struct(name, args) => self.struct_conformance_applies(name, args, tr),
            TyKind::Param { bounds, .. } => bounds
                .iter()
                .any(|bound| bound == tr || self.trait_refines(bound, tr)),
            _ => mojito_types::conformance::modelled_declares(ty, tr),
        }
    }

    /// A positive `conforms_to(T, Trait)` atom from the active method's
    /// availability clause refines only that opaque parameter, by its binder's
    /// identity, while its body is checked. No negative or disjunctive fact
    /// reaches this table (see `guaranteed_conformance_atoms`).
    pub(super) fn has_assumed_conformance(&self, ty: &Ty, required: &str) -> bool {
        let TyKind::Param { binder, .. } = ty.kind() else {
            return false;
        };
        let subject = AssumedSubject::Binder(binder.id.clone());
        self.assumed_conformances
            .borrow()
            .iter()
            .rev()
            .any(|scope| {
                scope.iter().any(|(parameter, available)| {
                    *parameter == subject
                        && (available == required
                            || self.trait_refines(available, required)
                            || builtin_trait_implies(available, required))
                })
            })
    }

    /// The conformance facts the elaborator decides a `where` clause or a
    /// `comptime if` from, as upstream decides a conformance when a clause
    /// asks it: the program's own traits with their parents, and each own
    /// struct's declared conformances and synthesized lifecycle rows. No
    /// struct is answered here for any other trait; a package struct's
    /// declaration carries the same facts.
    pub(super) fn conformance_facts(&self) -> mojito_checked::checked::ConformanceFacts {
        let mut traits: Vec<_> = self
            .traits
            .own()
            .iter()
            .map(
                |(name, info)| mojito_checked::declaration::TraitDeclaration {
                    name: name.clone(),
                    refines: info.refines.clone(),
                    requirements: info
                        .requirements
                        .iter()
                        .map(|(name, arity, overload)| {
                            let sig = &info.methods[name][*overload];
                            mojito_checked::declaration::TraitRequirement {
                                name: name.clone(),
                                arity: *arity,
                                dispatch: method_lowered_name("__trait_dispatch", name, sig, None),
                                param_names: sig.names.clone(),
                                param_types: sig.params.clone(),
                                param_conventions: sig.conventions.clone(),
                                variadic_index: sig.variadic_index,
                                required: sig.required.clone(),
                                positional_only: sig.positional_only,
                                keyword_only: sig.keyword_only,
                                variadic: sig.variadic.clone(),
                                kw_variadic: sig.kw_variadic.clone(),
                            }
                        })
                        .collect(),
                },
            )
            .collect();
        traits.sort_by(|left, right| left.name.cmp(&right.name));
        let own = self.structs.own();
        let structs = own
            .iter()
            .map(|(name, info)| {
                let rows = LIFECYCLE_TRAITS
                    .iter()
                    .filter_map(|required| {
                        let conditions =
                            self.struct_conformance_conditions(name, info, required)?;
                        Some(mojito_checked::checked::StructConformance {
                            trait_name: (*required).to_string(),
                            conditions,
                        })
                    })
                    .collect::<Vec<_>>();
                (name.clone(), rows)
            })
            .collect();
        let declared = own
            .iter()
            .map(|(name, info)| (name.clone(), self.declared_conformances(info)))
            .collect();
        let associated = own
            .iter()
            .map(|(name, info)| {
                let mut members: Vec<(String, Ty)> = info
                    .associated
                    .iter()
                    .filter_map(|(member, value)| match value {
                        CtValue::Type(ty) => Some((member.clone(), ty.clone())),
                        _ => None,
                    })
                    .collect();
                members.sort_by(|left, right| left.0.cmp(&right.0));
                (name.clone(), members)
            })
            .filter(|(_, members)| !members.is_empty())
            .collect();
        let associated_values = own
            .iter()
            .map(|(name, info)| {
                let mut members: Vec<(String, ParamExpr)> = info
                    .associated
                    .iter()
                    .filter(|(_, value)| !matches!(value, CtValue::Type(_)))
                    .filter_map(|(member, value)| {
                        let value = self.param_context.constant(value.clone()).ok()?;
                        Some((member.clone(), value))
                    })
                    .collect();
                members.sort_by(|left, right| left.0.cmp(&right.0));
                (name.clone(), members)
            })
            .filter(|(_, members)| !members.is_empty())
            .collect();
        let parameterized_associated = own
            .iter()
            .map(|(name, info)| {
                let mut members = info
                    .parameterized_associated
                    .iter()
                    .filter_map(|(member, declared)| declared.checked_fact(member))
                    .collect::<Vec<_>>();
                members.sort_by(|left, right| left.name.cmp(&right.name));
                (name.clone(), members)
            })
            .filter(|(_, members)| !members.is_empty())
            .collect();
        let parameterized_values = own
            .iter()
            .map(|(name, info)| {
                let mut members = info
                    .parameterized_associated
                    .iter()
                    .filter_map(|(member, declared)| declared.checked_value_fact(member))
                    .collect::<Vec<_>>();
                members.sort_by(|left, right| left.name.cmp(&right.name));
                (name.clone(), members)
            })
            .filter(|(_, members)| !members.is_empty())
            .collect();
        let fieldwise_conventions = own
            .iter()
            .filter(|(_, info)| {
                info.fieldwise_init
                    && !info
                        .fields
                        .iter()
                        .any(|(_, ty)| matches!(ty.kind(), TyKind::Ref(_)))
            })
            .map(|(name, info)| (name.clone(), self.fieldwise_conventions(&info.fields)))
            .collect();
        let witness_tables = own
            .iter()
            .filter(|(_, info)| !info.witness_tables.is_empty())
            .map(|(name, info)| (name.clone(), info.witness_tables.clone()))
            .collect();
        mojito_checked::checked::ConformanceFacts {
            traits,
            structs,
            declared,
            associated,
            parameterized_associated,
            associated_values,
            parameterized_values,
            fieldwise_conventions,
            witness_tables,
        }
    }

    /// The parameter conventions of the fieldwise `__init__` synthesized
    /// over `fields`, as upstream's `StructEmitter::synthesizeFieldwiseInit`
    /// declares them from each field type's register passability: a read
    /// for a register-passable-trivial field (`ImmReg`), whose copy is the
    /// value itself, and `var` (`OwnedMem`) for any other, so a copy
    /// happens in the caller, where a transfer or temporary elides it.
    pub(super) fn fieldwise_conventions(
        &self,
        fields: &[(String, Ty)],
    ) -> Vec<Option<mojito_ast::ast::ArgConvention>> {
        fields
            .iter()
            .map(|(_, ty)| {
                (!self.is_trivial_register_passable(ty))
                    .then_some(mojito_ast::ast::ArgConvention::Var)
            })
            .collect()
    }

    /// The struct's declared conformances, in declaration order, each with
    /// its compiled condition (`Bool(true)` when it states none). One whose
    /// condition does not compile proves nothing, so it is left out.
    fn declared_conformances(
        &self,
        info: &StructInfo,
    ) -> Vec<mojito_checked::checked::StructConformance> {
        info.conforms
            .iter()
            .filter_map(|declared| {
                let condition = match info.conformance_conditions.get(declared) {
                    Some(condition) => self.compile_condition(&info.decls, condition).ok()?,
                    None => GenericConstraint::Bool(true),
                };
                Some(mojito_checked::checked::StructConformance {
                    trait_name: declared.clone(),
                    conditions: vec![condition],
                })
            })
            .collect()
    }

    /// The conditions under which an instance of the struct `name` conforms
    /// to `required`, each over the struct's own binders: any one holding
    /// proves it, and none means no instance conforms. `required` may be an
    /// `IsTrivially*` spelling, whose row is the predicate's. `None` where
    /// the answer is not a function of the declared conformances (a type that
    /// answers by its elements before its specialization exists).
    fn struct_conformance_conditions(
        &self,
        name: &str,
        info: &StructInfo,
        required: &str,
    ) -> Option<Vec<GenericConstraint>> {
        let always = || vec![GenericConstraint::Bool(true)];
        let trivial = mojito_types::types::trivial_predicate_name(required);
        if info.decls.is_empty() {
            let self_ty = Ty::new(TyKind::Struct(name.to_string(), Vec::new()));
            let holds = trivial.map_or_else(
                || self.conforms_to(&self_ty, required),
                |kind| self.is_trivially(kind, &self_ty),
            );
            return Some(if holds { always() } else { Vec::new() });
        }
        let self_ty = Ty::new(TyKind::Struct(
            name.to_string(),
            params_as_args(&info.decls),
        ));
        if mojito_types::types::uninit_storage_element(&self_ty).is_some() {
            return None;
        }
        // The conditions of the declared conformances `accepts` admits. One
        // whose condition does not compile proves nothing.
        let declared = |accepts: &dyn Fn(&str) -> bool| -> Vec<GenericConstraint> {
            info.conforms
                .iter()
                .filter(|declared| accepts(declared))
                .filter_map(|declared| match info.conformance_conditions.get(declared) {
                    Some(condition) => self.compile_condition(&info.decls, condition).ok(),
                    None => Some(GenericConstraint::Bool(true)),
                })
                .collect()
        };
        let refining =
            |declared: &str| declared == required || self.trait_refines(declared, required);
        if let Some(kind) = trivial {
            let register_passable = declared(&|declared: &str| {
                declared == "TrivialRegisterPassable"
                    || self.trait_refines(declared, "TrivialRegisterPassable")
            });
            return self.trivial_lifecycle_conditions(name, info, kind, register_passable);
        }
        let lifecycle = match struct_trait_rule(required, self.traits.contains_key(required)) {
            StructTraitRule::Lifecycle => required,
            StructTraitRule::Declared => return Some(declared(&refining)),
            StructTraitRule::Never => return Some(Vec::new()),
            StructTraitRule::Shallow => return Some(always()),
        };
        Some(match lifecycle {
            "Copyable" => {
                let family = |declared: &str| {
                    matches!(
                        declared,
                        "Copyable" | "ImplicitlyCopyable" | "TrivialRegisterPassable"
                    )
                };
                if info.conforms.iter().any(|declared| family(declared)) {
                    declared(&family)
                } else if info.methods.contains_key("__copyinit__") {
                    always()
                } else {
                    Vec::new()
                }
            }
            "ImplicitlyCopyable" if self.struct_implicitly_copyable_conformance_ok(name, None) => {
                declared(&|declared| {
                    matches!(declared, "ImplicitlyCopyable" | "TrivialRegisterPassable")
                })
            }
            "ImplicitlyCopyable" => Vec::new(),
            _ => {
                if info.conforms.iter().any(|declared| declared == required) {
                    declared(&refining)
                } else {
                    always()
                }
            }
        })
    }

    /// The row of `IsTrivially*[Self]` for a generic struct, as
    /// [`Self::is_trivially`] answers it: the `register_passable` conditions
    /// of a declared `TrivialRegisterPassable`, or the base capability with
    /// no user lifecycle member for the facet and every field trivial.
    fn trivial_lifecycle_conditions(
        &self,
        name: &str,
        info: &StructInfo,
        kind: mojito_types::types::TrivialLifecycle,
        mut register_passable: Vec<GenericConstraint>,
    ) -> Option<Vec<GenericConstraint>> {
        use mojito_types::types::TrivialLifecycle;
        if user_lifecycle_defeats(info, kind) {
            return Some(register_passable);
        }
        let base = match kind {
            TrivialLifecycle::Movable => "Movable",
            TrivialLifecycle::Copyable => "Copyable",
            TrivialLifecycle::Deinitable => "Deinitable",
        };
        let capability = self
            .struct_conformance_conditions(name, info, base)?
            .into_iter()
            .reduce(|left, right| GenericConstraint::Or(Box::new(left), Box::new(right)));
        // A field whose facet no binder decides is folded here, as it holds
        // or fails at every instance: a concrete type answers either way,
        // and a scalar or pointer over a binder answers where its structure
        // proves the facet; one a binder decides stays a conjunct.
        let mut fieldwise = Some(capability?);
        for (_, field_ty) in &info.fields {
            let decided = if mojito_types::types::is_symbolic(field_ty) {
                (matches!(
                    field_ty.kind(),
                    TyKind::Simd { .. } | TyKind::Pointer { .. }
                ) && self.is_trivially(kind, field_ty))
                .then_some(true)
            } else {
                Some(self.is_trivially(kind, field_ty))
            };
            fieldwise = match decided {
                Some(true) => fieldwise,
                Some(false) => None,
                None => fieldwise.map(|joined| {
                    GenericConstraint::And(
                        Box::new(joined),
                        Box::new(GenericConstraint::Trivial(
                            kind,
                            ConstraintOperand::Type(field_ty.clone()),
                        )),
                    )
                }),
            };
        }
        register_passable.extend(fieldwise);
        Some(register_passable)
    }

    pub(super) fn struct_conformance_applies(
        &self,
        name: &str,
        args: &[TyArg],
        required: &str,
    ) -> bool {
        // A minted value specialization not registered here (the
        // specialization oracle predates the clone) answers as its template
        // applied to the baked values.
        let demangled;
        let (name, args) = match self.structs.get(name) {
            Some(_) => (name, args),
            None => match mojito_symbol::symbol::demangle_specialization(name) {
                Some((template, values)) if self.structs.contains_key(template) => {
                    demangled = values.into_iter().map(TyArg::Val).collect::<Vec<_>>();
                    (template, demangled.as_slice())
                }
                _ => return false,
            },
        };
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

    /// Whether a struct's conformance condition holds under `args`: the
    /// condition compiled against the struct's own binders, judged under the
    /// arguments bound to them by identity.
    pub(super) fn eval_conformance_condition(
        &self,
        info: &StructInfo,
        args: &[TyArg],
        expr: &Expr,
    ) -> bool {
        let Ok(condition) = self.compile_condition(&info.decls, expr) else {
            return false;
        };
        // A sole variadic parameter binds every positional argument as one
        // pack.
        let pack = positional_pack_binding(&info.decls, args).map(|(_, list)| list);
        let environment = match (&pack, info.decls.as_slice()) {
            (Some(list), [decl]) => ConstraintEnvironment::declared(
                std::slice::from_ref(decl),
                std::slice::from_ref(list),
            ),
            _ => ConstraintEnvironment::declared(&info.decls, args),
        };
        self.eval_generic_constraint(&condition, &environment)
    }

    pub(super) fn trait_refines(&self, candidate: &str, required: &str) -> bool {
        refines_trait(&self.traits, candidate, required)
    }

    /// Whether the trait `available` is `required` or implies it: every trait
    /// refines `AnyType`, beside its declared and built-in refinements. Every
    /// bound implies `!kgen.type`'s, which implies nothing else.
    pub(super) fn trait_implies(&self, available: &str, required: &str) -> bool {
        available == required
            || required == TYPE_OF_ALL_TYPES
            || (available != TYPE_OF_ALL_TYPES
                && (required == "AnyType"
                    || self.trait_refines(available, required)
                    || builtin_trait_implies(available, required)))
    }

    /// Explain the first actionable reason a built-in bound failed. This is
    /// intentionally evidence-oriented: marker traits name the field that
    /// prevents fieldwise synthesis, while operation traits name the operation
    /// promised by the bound.
    #[allow(
        clippy::format_push_string,
        reason = "TODO: write! into the buffer instead"
    )]
    pub(super) fn trait_failure_reason(&self, ty: &Ty, tr: &str) -> Option<String> {
        let TyKind::Struct(name, arguments) = ty.kind() else {
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
                let retired_leaf = info
                    .methods
                    .get("_update_with_simd")
                    .is_some_and(|methods| {
                        methods.iter().any(|method| {
                            method.params == [canonical_simd_ty(mojito_ast::ast::Dtype::UInt64, 1)]
                        })
                    });
                ["__init__", "_update_with_bytes", "_update_with_simd", "update", "finish"]
                    .into_iter()
                    .find(|method| !has(method))
                    .map(|method| format!("missing required Hasher member '{method}'"))
                    .or_else(|| {
                        retired_leaf.then(|| {
                            "'_update_with_simd(mut self, value: UInt64)' is the retired Mojito \
                             spelling; spell 'def _update_with_simd(mut self, value: SIMD[_, _])' \
                             and read the lanes with 'value.to_bits[DType.uint64]()'"
                                .to_string()
                        })
                    })
                    .or_else(|| {
                        Some(
                            "a Hasher member has the wrong shape (expected '_update_with_simd(mut self, SIMD[_, _])', \
                             '_update_with_bytes(mut self, Span[Byte, _])', 'update(mut self, Some[Hashable])', \
                             and 'finish(var self) -> UInt64')"
                                .to_string(),
                        )
                    })
            }
            "Indexer" => {
                let mut reason = format!(
                    "'{name}' does not implement all requirements for 'Indexer'\nnote: no \
                     '__mlir_index__' candidates have type 'def(self: {name}) thin -> \
                     __mlir_type.index'"
                );
                for method in info.methods.get("__mlir_index__").into_iter().flatten() {
                    reason.push_str(&format!(
                        "\nnote: candidate declared here with type 'def(self: {name}) thin -> {}'",
                        if method.ret_mlir_index {
                            "__mlir_type.index".to_string()
                        } else {
                            method.ret.to_string()
                        }
                    ));
                }
                Some(reason)
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
    /// `ImplicitlyCopyable` conformance **or defines `__copyinit__`**, and a type
    /// parameter only if bounded by Copyable/ImplicitlyCopyable.
    pub(super) fn is_copyable(&self, ty: &Ty) -> bool {
        if let Some(element) = self.opaque_element(ty) {
            return self.is_copyable(&element);
        }
        if self.has_assumed_conformance(ty, "Copyable")
            || self.has_assumed_conformance(ty, "ImplicitlyCopyable")
        {
            return true;
        }
        match ty.kind() {
            TyKind::Struct(name, args) => self.structs.get(name).is_none_or(|s| {
                // A declared Copyable-family conformance is the contract
                // and its condition decides: `Array[T]` declares
                // `Copyable where conforms_to(T, Copyable)` beside a
                // where-gated copy initializer, so the initializer's
                // presence alone proves nothing. Only a struct declaring
                // no such conformance answers by its copy initializer.
                let mut declared = s
                    .conforms
                    .iter()
                    .filter(|c| {
                        matches!(
                            c.as_str(),
                            "Copyable" | "ImplicitlyCopyable" | "TrivialRegisterPassable"
                        )
                    })
                    .peekable();
                if declared.peek().is_some() {
                    declared.any(|c| {
                        s.conformance_conditions.get(c).is_none_or(|condition| {
                            self.eval_conformance_condition(s, args, condition)
                        })
                    })
                } else {
                    s.methods.contains_key("__copyinit__")
                }
            }),
            TyKind::Param { bounds, .. } => bounds.iter().any(|bound| {
                matches!(
                    bound.as_str(),
                    "Copyable" | "ImplicitlyCopyable" | "TrivialRegisterPassable"
                ) || self.trait_refines(bound, "Copyable")
            }),
            // An abstract associated type (`C.Element`) is copyable only when
            // the bound trait's member declaration proves it — never
            // vacuously.
            TyKind::Assoc { .. } => self.assoc_member_bound_proves(ty, "Copyable"),
            // Scalars, `String`, `List`/`Tuple`/`Simd`/`Range`, `Error`, closures,
            // and `Self` are treated as copyable (element-wise copyability of
            // aggregates is not modeled).
            // Every other type answers by its shape, an aggregate by its
            // elements.
            _ => mojito_types::conformance::leaf_conforms(ty, "Copyable", &mut |element, _| {
                self.is_copyable(element)
            })
            .unwrap_or(true),
        }
    }

    /// `ImplicitlyCopyable` is stronger than `Copyable`: it means the type can be
    /// copied by the ordinary implicit copy path, not only by an explicit custom
    /// copy constructor. Structs opt in by declaring the marker, and fieldwise
    /// conformance requires all fields to be implicitly copyable.
    pub(super) fn is_implicitly_copyable(&self, ty: &Ty) -> bool {
        if let Some(element) = self.opaque_element(ty) {
            return self.is_implicitly_copyable(&element);
        }
        if self.has_assumed_conformance(ty, "ImplicitlyCopyable") {
            return true;
        }
        // Compiler-private inline uninit storage copies its raw bits; the
        // `MaybeUninit` header conditions gate the public copy on the
        // payload's triviality.
        if mojito_types::types::uninit_storage_element(ty).is_some() {
            return true;
        }
        match ty.kind() {
            TyKind::Struct(name, args) => self.structs.get(name).map_or_else(
                || {
                    mojito_types::types::list_element(ty).is_none()
                        && mojito_types::types::dict_elements(ty).is_none()
                        && mojito_types::types::set_element(ty).is_none()
                        && mojito_types::types::optional_element(ty).is_none()
                        && mojito_types::types::array_element(ty).is_none()
                        && mojito_types::types::owned_pointer_element(ty).is_none()
                },
                // A conditional conformance's condition decides at the
                // instance: its fields were checked under that condition at
                // the declaration. An unconditional one holds fieldwise.
                |s| {
                    s.conforms.iter().any(|c| {
                        matches!(c.as_str(), "ImplicitlyCopyable" | "TrivialRegisterPassable")
                            && s.conformance_conditions.get(c).map_or_else(
                                || self.struct_implicitly_copyable_conformance_ok(name, None),
                                |condition| self.eval_conformance_condition(s, args, condition),
                            )
                    })
                },
            ),
            TyKind::Param { bounds, .. } => bounds.iter().any(|bound| {
                matches!(
                    bound.as_str(),
                    "ImplicitlyCopyable" | "TrivialRegisterPassable"
                ) || self.trait_refines(bound, "ImplicitlyCopyable")
            }),
            // An abstract associated type is implicitly copyable only when its
            // declared member bounds say so, mirroring the `TyKind::Param` rule.
            TyKind::Assoc { .. } => self.assoc_member_bound_proves(ty, "ImplicitlyCopyable"),
            // Every other type answers by its shape, an aggregate by its
            // elements.
            _ => mojito_types::conformance::leaf_conforms(
                ty,
                "ImplicitlyCopyable",
                &mut |element, _| self.is_implicitly_copyable(element),
            )
            .unwrap_or(true),
        }
    }

    /// Whether a call binds an argument to a value independent of the caller's
    /// place, so it cannot conflict with a `mut`/`ref` access in the same call.
    /// A consuming slot copies the place out before the callee runs, whatever
    /// the type — `xs[i] = xs[j]`, `bump(mut s, var s)`, both of which the
    /// pinned Mojo accepts — and that copy is the one `check_consuming_as`
    /// records as `CopyPlaceValue`. A `^` transfer is the move the exclusivity
    /// rule itself reports, a `mut`/`ref` slot borrows, and every other slot is
    /// a shared read, which only a register-like value disarms. The carried-
    /// origin rule draws the same line in `origins::exclusivity::own_place`.
    pub(super) fn argument_is_independent_copy(
        &self,
        convention: Option<ArgConvention>,
        argument: &Expr,
        ty: &Ty,
    ) -> bool {
        if matches!(argument.kind, ExprKind::Transfer(_)) {
            return false;
        }
        match convention {
            Some(ArgConvention::Mut | ArgConvention::Ref) => false,
            Some(ArgConvention::Var | ArgConvention::Deinit) => true,
            _ => self.call_read_is_independent_copy(ty),
        }
    }

    /// Whether a shared call argument is materialized independently before
    /// within-call exclusivity is checked. Upstream keeps nominal memory values
    /// (including `String`) as overlapping reads even when their type conforms
    /// to `ImplicitlyCopyable`; register-like values may be disarmed as copies.
    pub(super) fn call_read_is_independent_copy(&self, ty: &Ty) -> bool {
        self.is_implicitly_copyable(ty) && !matches!(ty.kind(), TyKind::Struct(..))
    }

    /// Whether an associated type projected off a bounded parameter
    /// (`C.Element`) proves `required` through the bound trait's declared
    /// member bounds. `Copyable` is satisfied by `Copyable`,
    /// `ImplicitlyCopyable`, or a refining trait; `ImplicitlyCopyable`
    /// requires the exact marker, matching the `TyKind::Param` rule.
    fn assoc_member_bound_proves(&self, ty: &Ty, required: &str) -> bool {
        let TyKind::Assoc { .. } = ty.kind() else {
            return false;
        };
        self.projection_bounds(ty).is_some_and(|associated_bounds| {
            associated_bounds.iter().any(|bound| {
                bound == required
                    || (required == "Copyable"
                        && (bound == "ImplicitlyCopyable" || self.trait_refines(bound, "Copyable")))
            })
        })
    }

    /// Every initialized value of a concrete type is movable by default; only
    /// a struct that *declares* a conditional `Movable` conformance whose
    /// predicate fails (commonly `Movable where False`) opts out. A type
    /// parameter is movable only where its bounds prove it, as
    /// `is_deinitable` asks of `Deinitable`: the declaration is checked once
    /// for every instance, an opted-out one included.
    pub(super) fn is_movable(&self, ty: &Ty) -> bool {
        if let Some(element) = self.opaque_element(ty) {
            return self.is_movable(&element);
        }
        if self.has_assumed_conformance(ty, "Movable") {
            return true;
        }
        match ty.kind() {
            TyKind::Struct(name, args) => self.structs.get(name).is_none_or(|info| {
                if info.conforms.iter().any(|tr| tr == "Movable") {
                    self.struct_conformance_applies(name, args, "Movable")
                } else {
                    true
                }
            }),
            TyKind::Param {
                bounds,
                callable_bound: None,
                ..
            } => self.bounds_prove_movable(bounds),
            // Every other type answers by its shape, an aggregate by its
            // elements.
            _ => mojito_types::conformance::leaf_conforms(ty, "Movable", &mut |element, _| {
                self.is_movable(element)
            })
            .unwrap_or(true),
        }
    }

    /// Whether a parameter's bounds prove `Movable`: the trait itself, a
    /// trait refining it (`Copyable` and what refines that), or the
    /// trivial-lifecycle marker that implies it.
    pub(super) fn bounds_prove_movable(&self, bounds: &[String]) -> bool {
        bounds.iter().any(|bound| {
            matches!(
                bound.as_str(),
                "Movable" | "Copyable" | "ImplicitlyCopyable" | "TrivialRegisterPassable"
            ) || self.trait_refines(bound, "Movable")
        })
    }

    /// Whether `ty` conforms to `TrivialRegisterPassable`: a numeric, `Bool`,
    /// `DType`, or SIMD-valued builtin, a struct declaring the conformance, or a
    /// parameter bounded or assumed by it. The general `conforms_to` answers
    /// marker traits shallowly, so it cannot be asked.
    pub(super) fn is_trivial_register_passable(&self, ty: &Ty) -> bool {
        if let Some(element) = self.opaque_element(ty) {
            return self.is_trivial_register_passable(&element);
        }
        super::builtins::is_numeric(ty)
            || matches!(ty.kind(), TyKind::Bool | TyKind::Dtype)
            || super::builtins::simd_valued_ty(ty)
            || match ty.kind() {
                TyKind::Struct(name, args) => {
                    self.struct_conformance_applies(name, args, "TrivialRegisterPassable")
                }
                TyKind::Param { bounds, .. } => {
                    bounds
                        .iter()
                        .any(|bound| bound == "TrivialRegisterPassable")
                        || self.has_assumed_conformance(ty, "TrivialRegisterPassable")
                }
                _ => false,
            }
    }

    /// Whether `ty` conforms to `RegisterPassable`: a trivially
    /// register-passable type (whose trait refines it), a struct whose
    /// declared conformance holds, or a parameter bounded or assumed by it.
    /// A struct whose conditional conformance does not hold, as upstream
    /// judges it whether or not its arguments are closed, and any other
    /// parameter or abstract type, only might.
    pub(super) fn register_passability(&self, ty: &Ty) -> RegisterPassability {
        use RegisterPassability::{Excluded, Possible, Proven};
        if let Some(element) = self.opaque_element(ty) {
            return self.register_passability(&element);
        }
        if self.is_trivial_register_passable(ty) {
            return Proven;
        }
        let proves = |bound: &str| {
            matches!(bound, "RegisterPassable" | "TrivialRegisterPassable")
                || self.trait_refines(bound, "RegisterPassable")
                || self.trait_refines(bound, "TrivialRegisterPassable")
        };
        match ty.kind() {
            TyKind::StringLiteral | TyKind::None => Proven,
            TyKind::Tuple(elements) => elements.iter().fold(Proven, |joined, element| {
                match (joined, self.register_passability(element)) {
                    (Excluded, _) | (_, Excluded) => Excluded,
                    (Possible, _) | (_, Possible) => Possible,
                    (Proven, Proven) => Proven,
                }
            }),
            TyKind::Struct(name, args) => {
                if self.struct_conformance_applies(name, args, "RegisterPassable")
                    || self.struct_conformance_applies(name, args, "TrivialRegisterPassable")
                {
                    return Proven;
                }
                let conditional = self.structs.get(name).is_some_and(|info| {
                    info.conforms.iter().any(|declared| {
                        proves(declared) && info.conformance_conditions.contains_key(declared)
                    })
                });
                if conditional { Possible } else { Excluded }
            }
            TyKind::Param { bounds, .. } => {
                if bounds.iter().any(|bound| proves(bound))
                    || self.has_assumed_conformance(ty, "RegisterPassable")
                {
                    Proven
                } else {
                    Possible
                }
            }
            TyKind::Assoc { .. } | TyKind::SelfType | TyKind::Dependent(_) => Possible,
            _ => Excluded,
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
        let trp_conforms = match ty.kind() {
            TyKind::Struct(name, args) => {
                self.struct_conformance_applies(name, args, "TrivialRegisterPassable")
            }
            TyKind::Param { bounds, .. } => {
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
        match ty.kind() {
            TyKind::ComptimeList(element) => self.trivial_lifecycle(kind, element, visiting),
            TyKind::Tuple(elements) | TyKind::RuntimePack(elements) => elements
                .iter()
                .all(|element| self.trivial_lifecycle(kind, element, visiting)),
            TyKind::Variant(alternatives) => alternatives
                .iter()
                .all(|alternative| self.trivial_lifecycle(kind, alternative, visiting)),
            TyKind::Struct(name, args) => {
                if !visiting.insert(name.clone()) {
                    // A cycle can only occur through indirection; the pointer
                    // value itself is trivial.
                    return true;
                }
                let result = self.structs.get(name).is_some_and(|info| {
                    // Fields are stored at the declaration's parameters;
                    // recurse at this instantiation's arguments so a generic
                    // payload field answers for the concrete element.
                    !user_lifecycle_defeats(info, kind)
                        && info.fields.iter().all(|(_, field_ty)| {
                            let field_ty = substitute_at(field_ty, info, args);
                            self.trivial_lifecycle(kind, &field_ty, visiting)
                        })
                });
                visiting.remove(name);
                result
            }
            // Generic parameters and associated members have no structural
            // proof; comptime sites see concrete types after specialization.
            TyKind::Param { .. } | TyKind::SelfType | TyKind::Infer => false,
            // Scalars, literals, pointers-as-values, and the remaining
            // primitive representations move/copy bitwise and drop as no-ops.
            _ => true,
        }
    }

    pub(super) fn is_deinitable(&self, ty: &Ty) -> bool {
        if let Some(element) = self.opaque_element(ty) {
            return self.is_deinitable(&element);
        }
        if self.has_assumed_conformance(ty, "Deinitable") {
            return true;
        }
        match ty.kind() {
            TyKind::Struct(name, args) => self.structs.get(name).is_none_or(|info| {
                if info.conforms.iter().any(|tr| tr == "Deinitable") {
                    self.struct_conformance_applies(name, args, "Deinitable")
                } else {
                    true
                }
            }),
            TyKind::Param { binder, bounds, .. } => {
                self.trait_bounded_binders.contains_key(&binder.id)
                    || self.bounds_prove_deinitable(bounds)
            }
            // Every other type answers by its shape, an aggregate by its
            // elements.
            _ => mojito_types::conformance::leaf_conforms(ty, "Deinitable", &mut |element, _| {
                self.is_deinitable(element)
            })
            .unwrap_or(true),
        }
    }

    /// Whether a parameter's bounds prove `Deinitable`: the trait itself, a
    /// trait refining it, or the trivial-lifecycle marker that implies it.
    pub(super) fn bounds_prove_deinitable(&self, bounds: &[String]) -> bool {
        bounds.iter().any(|bound| {
            bound == "Deinitable"
                || bound == "TrivialRegisterPassable"
                || self.trait_refines(bound, "Deinitable")
        })
    }

    pub(super) fn is_hashable(&self, ty: &Ty) -> bool {
        if let Some(element) = self.opaque_element(ty) {
            return self.is_hashable(&element);
        }
        if self.has_assumed_conformance(ty, "Hashable") {
            return true;
        }
        match ty.kind() {
            TyKind::Struct(name, args) => self.struct_conformance_applies(name, args, "Hashable"),
            TyKind::Param { bounds, .. } => bounds.iter().any(|b| b == "Hashable"),
            _ => builtin_hashable_ty(ty),
        }
    }

    pub(super) fn is_comparable(&self, ty: &Ty) -> bool {
        if self.has_assumed_conformance(ty, "Comparable") {
            return true;
        }
        match ty.kind() {
            TyKind::Struct(name, args) => self.struct_conformance_applies(name, args, "Comparable"),
            TyKind::Param { bounds, .. } => bounds.iter().any(|b| b == "Comparable"),
            _ => is_numeric_like(ty) || is_scalar_simd(ty),
        }
    }

    /// `assumption` is a conditional conformance's premise: a field of a
    /// parameter type the premise makes copyable counts as copyable.
    pub(super) fn struct_copyable_conformance_ok(
        &self,
        name: &str,
        assumption: Option<&GenericConstraint>,
    ) -> bool {
        let Some(info) = self.structs.get(name) else {
            return false;
        };
        info.methods.contains_key("__copyinit__")
            || info.fields.iter().all(|(_, ty)| {
                self.is_copyable_under_assumption(ty, assumption)
                    // The focused checker (no linked prelude) sees a
                    // `List[Int]`/`Tuple[*Ts]` field as an unregistered
                    // nominal spelling; the ordinary capability helper's
                    // compatibility default answers for it.
                    || matches!(ty.kind(), TyKind::Struct(field_struct, _)
                        if !self.structs.contains_key(field_struct) && self.is_copyable(ty))
            })
    }

    pub(super) fn struct_implicitly_copyable_conformance_ok(
        &self,
        name: &str,
        assumption: Option<&GenericConstraint>,
    ) -> bool {
        let Some(info) = self.structs.get(name) else {
            return false;
        };
        info.methods.contains_key("__copyinit__")
            || info.fields.iter().all(|(_, ty)| {
                self.is_implicitly_copyable(ty)
                    // Private pack storage (`__RuntimeTuple[*Self.Ts]`,
                    // copies as its elements do, which the conformance's own
                    // `where` clause states.
                    || matches!(ty.kind(), TyKind::Tuple(elements) | TyKind::Variant(elements)
                        if matches!((mojito_types::types::pack_spread(elements)).map(Ty::kind),
                            Some(TyKind::Param { binder, .. }) if assumption.is_some_and(|known| {
                                binder_conformance_assumed(binder, "ImplicitlyCopyable", known)
                            })))
            })
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

    /// The arguments an owned `var *args` collector gathers at `positions`
    /// are consumed like a named `var` parameter's: a place is copied into
    /// the pack. A forwarded pack is bound whole, against the collector's
    /// convention. A collector that only reads its elements copies nothing.
    pub(super) fn check_consuming_collected(
        &self,
        owned: bool,
        positions: &[usize],
        args: &[Expr],
        callee: &str,
    ) -> Result<(), TypeError> {
        positions
            .iter()
            .map(|&position| &args[position])
            .filter(|argument| {
                owned && self.designates_place(argument) && self.forwarded_pack(argument).is_none()
            })
            .try_for_each(|argument| {
                let ty = self.infer(argument)?;
                self.check_consuming(argument, &ty, &format!("variadic argument to {callee}"))
            })
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
            let reason = if matches!(ty.kind(), TyKind::Param { .. }) {
                "a value of a parameter type transfers only where a bound proves 'Movable'"
            } else {
                "its 'Movable' conformance condition is false"
            };
            return Err(TypeError::TraitNotSatisfied {
                param: context.to_string(),
                ty: ty.to_string(),
                trait_name: "Movable".to_string(),
                reason: self
                    .trait_failure_reason(ty, "Movable")
                    .or_else(|| Some(reason.to_string())),
            });
        }
        // A transfer of a Movable value and a fresh temporary (a call result,
        // a literal, an operator) move freely; a *place* must be Copyable.
        if self.designates_place(expr) {
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
            // A field chain off a compile-time binding is a fresh value its
            // consumer takes, no place a copy is made of.
            let parameter_field = !matches!(expr.kind, ExprKind::Identifier(_))
                && self.is_parameter_read(expr)
                && self.is_materialized_parameter_read(expr);
            if !parameter_field {
                self.record_at(expr.source_span()).copy_place_value_use = true;
            }
            self.consume_parameter_read(expr);
        }
        Ok(())
    }

    /// A compile-time initializer is its thunk's `return` operand: a
    /// projection or a reference result of a `Copyable` value is copied
    /// out of the temporaries the thunk drops. The pin's compile-time
    /// materialization asks `Copyable`, not `ImplicitlyCopyable`; any
    /// other value is returned as it stands.
    pub(super) fn copy_compile_time_result(&self, value: &Expr, ty: &Ty) {
        if !self.is_copyable(ty) {
            return;
        }
        let span = value.source_span();
        let reference_result = matches!(
            self.node_facts
                .borrow()
                .get(&span)
                .and_then(|facts| facts.adjustment.as_deref()),
            Some(mojito_checked::checked::SemanticAdjustment::ReferenceResult { .. })
        );
        if reference_result {
            self.copyable_reference_result_reads
                .borrow_mut()
                .insert(span);
        } else if self.designates_place(value) {
            self.record_at(span).copy_place_value_use = true;
        }
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
                vec![
                    TyKind::Param {
                        binder: synthetic_binder("Some[Hasher]"),
                        bounds: vec!["Hasher".to_string()],
                        callable_bound: None,
                    }
                    .into(),
                ],
                Ty::none(),
            );
            signature.conventions[0] = Some(ArgConvention::Mut);
            methods.push(signature);
        }
        if bounds.iter().any(|bound| bound == "Hasher") {
            let signature = match (method, argc) {
                ("update", 1) => Some((
                    TyKind::Param {
                        binder: synthetic_binder("Some[Hashable]"),
                        bounds: vec!["Hashable".to_string()],
                        callable_bound: None,
                    },
                    TyKind::None,
                    Some(ArgConvention::Mut),
                )),
                _ => None,
            };
            if let Some((param, ret, convention)) = signature {
                let mut signature = MethodSig::intrinsic(vec![param.into()], ret.into());
                signature.self_convention = convention;
                methods.push(signature);
            }
            if method == "_update_with_simd" && argc == 1 {
                methods.push(hasher_simd_update_requirement());
            }
            if method == "finish" && argc == 0 {
                let mut signature = MethodSig::intrinsic(
                    vec![],
                    canonical_simd_ty(mojito_ast::ast::Dtype::UInt64, 1),
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
            methods.push(MethodSig::intrinsic(vec![], Ty::self_type()));
        }
        // The built-in numeric-rounding traits contribute a `-> Self` dunder
        // (roadmap milestone 7), used by the self-hosted `math` module: `Floorable`/
        // `Ceilable`/`Truncable` a nullary `__floor__`/`__ceil__`/`__trunc__`,
        // and `CeilDivable`/`CeilDivableRaising` a unary `__ceildiv__(Self)`.
        let accepts = math_dunder_bound(method, argc);
        if !accepts.is_empty() && bounds.iter().any(|b| accepts.contains(&b.as_str())) {
            let params = if argc == 1 {
                vec![Ty::self_type()]
            } else {
                vec![]
            };
            methods.push(MethodSig::intrinsic(params, Ty::self_type()));
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

    /// The bounds an archetype conforms to: a type parameter's declared
    /// bounds, or an associated projection's (`C.IteratorType[o]`, nested
    /// `C.IteratorType[o].Element`) as its declaring trait requires them.
    pub(super) fn projection_bounds(&self, ty: &Ty) -> Option<Vec<String>> {
        match ty.kind() {
            TyKind::Param { bounds, .. } => Some(bounds.clone()),
            TyKind::Assoc { base, name, .. } => {
                self.lookup_trait_assoc_type(&self.projection_bounds(base)?, name)
            }
            _ => None,
        }
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

    /// The parameterized value requirement `member` of a bound trait
    /// (`comptime get[idx: Int]: Int`): its declared parameters and result
    /// type.
    pub(super) fn lookup_trait_generator(
        &self,
        bounds: &[String],
        member: &str,
    ) -> Option<(&[ParamDecl], &Ty)> {
        bounds
            .iter()
            .filter_map(|b| self.traits.get(b))
            .find_map(|info| match info.comptime_members.get(member) {
                Some(CtMemberReq::Generator { params, result }) => {
                    Some((params.as_slice(), result))
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
                Some(CtMemberReq::Value(ty)) => Some(ty.clone()),
                _ => None,
            })
    }

    /// A requirement's default as the attribute its signature holds, the
    /// same at a call through the bound as in the trait's scope: a literal,
    /// a module constant, or a compile-time `Int` expression over them, as
    /// the constant it names there; a default reading the method's own value
    /// parameters as a parameter expression over them, module constants
    /// folded, which a call through the bound evaluates with its binding of
    /// them (`bound_defaults.rs`). The requirement's binders are the
    /// innermost scope.
    fn requirement_default(
        &self,
        default: &Expr,
        decls: &[ParamDecl],
    ) -> Result<SignatureDefault, TypeError> {
        if literal_default(default) {
            return mojito_checked::checked::CheckedConst::from_expr(default)
                .map(SignatureDefault::Constant)
                .ok_or_else(unfoldable_requirement_default);
        }
        if reads_value_parameter(default, decls) {
            if !self.parameter_default_shape(default, decls) {
                return Err(unfoldable_requirement_default());
            }
            return self
                .compile_dependent_ct_expr(default)
                .map(SignatureDefault::Parameter)
                .map_err(|_| unfoldable_requirement_default());
        }
        if decls.iter().any(|decl| reads_name(default, decl.name())) {
            return Err(unfoldable_requirement_default());
        }
        self.folded_constant(default)
            .map(SignatureDefault::Constant)
            .ok_or_else(unfoldable_requirement_default)
    }

    /// A module constant, or a compile-time `Int` expression over them, as
    /// the constant it names in the trait's scope.
    fn folded_constant(&self, default: &Expr) -> Option<mojito_checked::checked::CheckedConst> {
        if let ExprKind::Identifier(name) = &default.kind
            && let Some(constant) = self.comptime_literals.get(name)
        {
            return mojito_checked::checked::CheckedConst::from_expr(constant);
        }
        self.eval_ct(default)
            .ok()
            .map(mojito_checked::checked::CheckedConst::Int)
    }

    /// Whether a default is an expression over the method's value
    /// parameters, literals, and module constants alone.
    /// A trait's parameterized value requirement as the generator type it
    /// declares: its parameters classified under `Trait.member`, beside its
    /// `result`. Origin parameters are rejected, as on a struct's
    /// parameterized value member.
    fn generator_requirement(
        &mut self,
        trait_name: &str,
        member: &TraitComptime,
        result: Ty,
    ) -> Result<CtMemberReq, TypeError> {
        if member.params.iter().any(|parameter| {
            parameter.is_origin_mutability_binder(&member.params)
                || matches!(
                    constraints::assoc_param_kind(parameter),
                    constraints::AssocParamKind::Origin
                )
        }) {
            return Err(TypeError::Unsupported(
                "origin parameters on a parameterized comptime value".to_string(),
            ));
        }
        let owner = format!("{}.{}", binder_owner(trait_name), member.name);
        let mut params = self.classify_params(&owner, &member.params)?;
        // The requirement's `where` clauses close its parameter list, as a
        // generic alias's do, so an application through a bound checks them.
        for condition in &member.where_clauses {
            let constraint = self.compile_where_clause(condition)?.bind(&[&params]);
            if let Some(
                ParamDecl::Type { constraints, .. } | ParamDecl::Value { constraints, .. },
            ) = params.last_mut()
            {
                constraints.push(constraint);
            }
        }
        Ok(CtMemberReq::Generator { params, result })
    }

    fn parameter_default_shape(&self, default: &Expr, decls: &[ParamDecl]) -> bool {
        match &default.kind {
            ExprKind::Identifier(name) => match decls.iter().find(|decl| decl.name() == name) {
                Some(ParamDecl::Value { .. }) => true,
                Some(ParamDecl::Type { .. }) => false,
                None => self.folded_constant(default).is_some(),
            },
            ExprKind::Prefix(_, operand) => self.parameter_default_shape(operand, decls),
            ExprKind::Infix(_, left, right) => {
                self.parameter_default_shape(left, decls)
                    && self.parameter_default_shape(right, decls)
            }
            _ => literal_default(default),
        }
    }
}

/// Whether the declared trait `candidate` refines `required`, directly or
/// through its parents.
pub(super) fn refines_trait(
    traits: &declaration_table::DeclarationTable<TraitInfo>,
    candidate: &str,
    required: &str,
) -> bool {
    mojito_types::conformance::refines(
        &|name| traits.get(name).map(|info| info.refines.as_slice()),
        candidate,
        required,
    )
}

/// Origin parameters a method body writes through via a parametric-mut ref
/// field subscript (`self.<field>[...] = v` / `+=`). The scan is syntactic
/// and complete for the accepted subset: only the direct self-rooted field
/// subscript form is a legal write target through such a field (alias forms
/// stay rejected by the reborrow rules), so no checked facts are needed.
/// Whether a parameterized value member's `body` is of the generator
/// requirement's `result` type: its value type coerces to it, or its
/// literal materializes as it (`comptime get[idx: Int] = 8` for `Int`).
fn generator_result_satisfies(body: &ParamExpr, result: &Ty) -> bool {
    match body.as_constant() {
        Some(value) => value.clone().materialize_as(result).is_some(),
        None => body
            .meta()
            .as_value()
            .is_some_and(|actual| coerces(actual, result)),
    }
}

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
                        && let Some((_, field_ty)) = fields.iter().find(|(name, _)| name == field)
                        && let TyKind::Ref(reference) = field_ty.kind()
                        && matches!(reference.mutability, Mutability::Param(_))
                        && let Origin::Param(id) = reference.origin.clone()
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
                StmtKind::ComptimeFor { body, .. }
                | StmtKind::With { body, .. }
                | StmtKind::Scope(body) => {
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
pub(super) struct SavedStructScope {
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

/// Whether a method satisfies upstream's `Indexer` requirement:
/// `def __mlir_index__(self) -> __mlir_type.index`.
pub(super) fn is_indexer_requirement(method: &MethodSig) -> bool {
    method.has_self
        && method.params.is_empty()
        && method.ret == TyKind::Int
        && method.ret_mlir_index
}

/// Whether a default is a literal, or a negated numeric one: an expression
/// that names nothing, so a trait requirement and its witness mean the same
/// value by the same spelling.
pub(super) fn literal_default(default: &mojito_ast::ast::Expr) -> bool {
    use mojito_ast::ast::{ExprKind, PrefixOp};
    match &default.kind {
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::Str(_) => true,
        ExprKind::None => true,
        ExprKind::Prefix(PrefixOp::Neg, operand) => {
            matches!(operand.kind, ExprKind::Int(_) | ExprKind::Float(_))
        }
        _ => false,
    }
}

/// Whether a default reads one of the requirement's own value parameters.
pub(super) fn reads_value_parameter(default: &mojito_ast::ast::Expr, decls: &[ParamDecl]) -> bool {
    decls
        .iter()
        .filter(|decl| matches!(decl, ParamDecl::Value { .. }))
        .any(|decl| reads_name(default, decl.name()))
}

/// Whether a parameter of `params` is typed by another declaration's member
/// (`values: _MLIR.KGENParamListType[type]`), which classifies only once
/// that declaration's member types resolve.
pub(super) fn names_member_type(params: &[mojito_ast::ast::TypeParam]) -> bool {
    params.iter().any(|param| {
        matches!(
            param.value_type,
            Some(SourceType::Assoc { .. } | SourceType::IndexedProjection { .. })
        )
    })
}

fn unfoldable_requirement_default() -> TypeError {
    TypeError::Unsupported(
        "a trait requirement's default value other than an expression over literals, module constants, and the method's own value parameters".to_string(),
    )
}

/// Whether a compile-time `Int` default reads `name`.
fn reads_name(default: &mojito_ast::ast::Expr, name: &str) -> bool {
    use mojito_ast::ast::ExprKind;
    match &default.kind {
        ExprKind::Identifier(read) => read == name,
        ExprKind::Prefix(_, operand) => reads_name(operand, name),
        ExprKind::Infix(_, left, right) => reads_name(left, name) || reads_name(right, name),
        _ => false,
    }
}

/// Whether the struct declares the lifecycle member a trivial `kind` rules
/// out: a user move, copy, or destroy.
fn user_lifecycle_defeats(info: &StructInfo, kind: mojito_types::types::TrivialLifecycle) -> bool {
    use mojito_types::types::TrivialLifecycle;
    match kind {
        TrivialLifecycle::Movable => info.methods.contains_key("__moveinit__"),
        TrivialLifecycle::Copyable => info.methods.contains_key("__copyinit__"),
        // A named destructor (`def close(deinit self)`) is a consuming
        // method beside the compiler-generated `__del__`, which stays
        // trivial, as the pin reads `MaybeUninit`'s.
        TrivialLifecycle::Deinitable => info.methods.contains_key("__deinit__"),
    }
}

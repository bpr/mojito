//! The `ConformanceOracle` implementation.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

/// A struct of an imported package, as the package's check resolved it,
/// which the elaborator reads in place of its declaration.
#[derive(Clone, Copy)]
pub struct PackageStruct<'a>(&'a StructInfo);

impl<'a> PackageStruct<'a> {
    /// Its compile-time parameters.
    pub fn decls(self) -> &'a [ParamDecl] {
        &self.0.decls
    }

    /// Its parameter list as declared, origin parameters and defaults
    /// included.
    pub fn source_params(self) -> &'a [mojito_ast::ast::TypeParam] {
        &self.0.source_params
    }

    /// Its declared field names, in order.
    pub fn field_names(self) -> &'a [String] {
        &self.0.declared_field_names
    }
}

impl ConformanceOracle {
    /// # Panics
    ///
    /// Panics if a trait recorded during collection is missing when its
    /// conformances are resolved.
    pub fn from_program(stmts: &[Stmt]) -> Result<Self, TypeError> {
        Self::importing(stmts, None)
    }

    /// The oracle of a program that imports the package declarations
    /// `installed` holds: they are attached, so a lookup the program's own
    /// entries miss resolves the build's declaration record on first use,
    /// shared with every checker pass of the compilation, and only `stmts`,
    /// the program's own, register from source.
    ///
    /// # Errors
    ///
    /// A declaration of `stmts` the registry cannot classify, or a name
    /// `installed` declares twice.
    ///
    /// # Panics
    ///
    /// As [`Self::from_program`].
    pub fn importing(
        stmts: &[Stmt],
        installed: Option<&InstalledInterfaces>,
    ) -> Result<Self, TypeError> {
        let mut checker = Checker::new();
        if let Some(installed) = installed {
            checker.attach_interfaces(installed)?;
        }
        // A field type may apply a module function to a parameter, whose
        // symbol spells the module constants the check folds.
        checker.fold_module_constants(stmts);
        let applicable = super::comptime_validation::applicable_functions(
            stmts,
            &mojito_symbol::symbol::OverloadSets::scan(stmts),
            &|name| checker.program_constants.get(name),
        );
        checker.applicable_functions.extend(applicable);

        // Refinement is the only trait fact needed by `conforms_to`. Register
        // every name first so the oracle is independent of body checking and
        // can answer nominal queries while specialization is still rewriting
        // the program.
        for statement in stmts {
            let StmtKind::Trait { name, refines, .. } = &statement.kind else {
                continue;
            };
            checker.traits.insert(
                name.clone(),
                TraitInfo {
                    refines: refines.clone(),
                    methods: HashMap::new(),
                    comptime_members: HashMap::new(),
                    comptime_constraints: HashMap::new(),
                    defaults: Vec::new(),
                    requirements: Vec::new(),
                },
            );
        }

        // Struct facts are likewise signature-only. Full conformance
        // verification still runs after elaboration, so accepting a declaration
        // into this registry never bypasses method or associated-member checks.
        // Every struct name registers before any field type resolves, so a
        // field may reference a struct declared later in the module (an
        // iterator holding `ref[o] List[T]` above `List` itself).
        // A struct whose parameter defaults name a comptime type alias
        // (`H: Hasher = default_hasher`) classifies only once the alias is
        // registered, and an alias body names structs — so structs register in
        // two passes around the alias pass, deferring the ones whose
        // classification fails the first time.
        let mut deferred_structs = Vec::new();
        let register_struct = |checker: &mut Checker,
                               statement: &Stmt,
                               defer: Option<&mut Vec<usize>>,
                               index: usize|
         -> Result<(), TypeError> {
            let StmtKind::Struct {
                name,
                type_params,
                conforms,
                fields,
                conformance_conditions,
                methods,
                fieldwise_init,
                ..
            } = &statement.kind
            else {
                return Ok(());
            };

            let decls = match checker.classify_params(name, type_params) {
                Ok(decls) => decls,
                Err(error) => match defer {
                    Some(deferred) => {
                        deferred.push(index);
                        return Ok(());
                    }
                    None => return Err(error),
                },
            };
            let mut method_names: HashMap<String, Vec<MethodSig>> = HashMap::new();
            for method in methods {
                method_names
                    .entry(lifecycle_method_name(method).to_string())
                    .or_default();
            }
            checker.structs.insert(
                name.clone(),
                StructInfo {
                    decls,
                    source_params: type_params.clone(),
                    conforms: conforms.clone(),
                    callable_conformance: None,
                    callable_target: None,
                    conformance_conditions: conformance_conditions.iter().cloned().collect(),
                    fields: Vec::new(),
                    declared_field_names: fields.iter().map(|field| field.name.clone()).collect(),
                    field_origin_arguments: HashMap::new(),
                    associated: HashMap::new(),
                    associated_constraints: HashMap::new(),
                    parameterized_associated: HashMap::new(),
                    methods: method_names,
                    fieldwise_init: *fieldwise_init,
                    explicit_destroy_message: None,
                    explicit_destructors: HashMap::new(),
                },
            );
            Ok(())
        };
        for (index, statement) in stmts.iter().enumerate() {
            register_struct(&mut checker, statement, Some(&mut deferred_structs), index)?;
        }
        // Best-effort generic comptime alias registration, so a struct
        // `where` clause compiled below can reference a predicate alias. A
        // body the signature-only registry cannot lower (e.g. one naming a
        // type this oracle never registers) is skipped: the full checker
        // still validates every declaration, and a condition referencing a
        // skipped alias fails closed at its lazy evaluation site.
        for statement in stmts {
            let StmtKind::Comptime {
                name,
                type_params,
                ty,
                where_clauses,
                value,
            } = &statement.kind
            else {
                continue;
            };
            // A single non-scalar bracket argument parses as indexing
            // (`AHasher[SIMD[DType.uint64, 4](0)]`), so that shape is a type
            // alias body too.
            if type_params.is_empty()
                && !matches!(
                    value.kind,
                    ExprKind::Identifier(_)
                        | ExprKind::TypeApply { .. }
                        | ExprKind::TypeValue(_)
                        | ExprKind::Index { .. }
                )
            {
                continue;
            }
            let _ = checker.check_generic_comptime_alias(
                name,
                type_params,
                ty.as_ref(),
                where_clauses,
                value,
            );
        }
        for index in deferred_structs {
            register_struct(&mut checker, &stmts[index], None, index)?;
        }
        // Struct `where` clauses compile after alias registration and attach
        // to the registered declaration's final parameter (or validate
        // immediately for a non-generic struct), as in the full checker.
        for statement in stmts {
            let StmtKind::Struct {
                name,
                type_params,
                where_clauses,
                ..
            } = &statement.kind
            else {
                continue;
            };
            for condition in where_clauses {
                let constraint = checker.compile_where_clause(condition)?;
                let info = checker
                    .structs
                    .get_mut(name)
                    .expect("struct was registered by the loop above");
                let constraint = constraint.bind(&[&info.decls]);
                if let Some(last) = info.decls.last_mut() {
                    match last {
                        ParamDecl::Type { constraints, .. }
                        | ParamDecl::Value { constraints, .. } => constraints.push(constraint),
                    }
                } else if type_params.is_empty() {
                    checker.validate_declaration_constraint(name, &constraint)?;
                }
            }
        }
        for statement in stmts {
            let StmtKind::Struct {
                name,
                type_params,
                fields,
                associated,
                conformance_conditions,
                methods,
                ..
            } = &statement.kind
            else {
                continue;
            };

            let decls = checker
                .structs
                .get(name)
                .map(|info| info.decls.clone())
                .unwrap_or_default();
            if decls.iter().any(|decl| {
                matches!(
                    decl,
                    ParamDecl::Type { variadic: true, .. }
                        | ParamDecl::Value { variadic: true, .. }
                ) || matches!(decl, ParamDecl::Value { ty, .. }
                    if matches!(*ty.kind(), TyKind::Dtype | TyKind::Struct(..)))
            }) {
                // Pack-dependent fields are expanded into ordinary concrete
                // fields/types by specialization; DType-/struct-valued
                // templates fold their fields the same way. The template
                // itself cannot be resolved as a single erased type.
                continue;
            }
            let self_ty = TyKind::Struct(name.clone(), self_struct_arguments(&decls, type_params));
            let saved_self_decls = std::mem::replace(&mut checker.self_decls, decls);
            let saved_type_params =
                std::mem::replace(&mut checker.enclosing_type_params, type_params.clone());
            let saved_self_ty = checker.self_ty.replace(self_ty.into());
            let saved_bundled = std::mem::replace(
                &mut checker.bundled_stdlib_declaration,
                is_bundled_stdlib_source(bundled_struct_source(
                    statement.module.as_deref(),
                    methods,
                )),
            );
            // Best-effort associated-member lowering BEFORE field resolution,
            // so a field type may apply the struct's own comptime alias
            // (`var iter: Self.dict_entry_iter`). A body this signature-only
            // oracle cannot lower is skipped and fails closed at its use
            // site, exactly like the generic-alias registration above.
            if let Ok((associated_values, associated_constraints, parameterized)) =
                checker.check_struct_associated(associated, conformance_conditions)
                && let Some(info) = checker.structs.get_mut(name)
            {
                info.associated = associated_values;
                info.associated_constraints = associated_constraints;
                info.parameterized_associated = parameterized;
            }
            let field_types = fields
                .iter()
                .map(|field| {
                    checker
                        .ty_from_anno(&field.ty)
                        .map(|ty| (field.name.clone(), ty))
                })
                .collect::<Result<Vec<_>, _>>();
            checker.self_decls = saved_self_decls;
            checker.enclosing_type_params = saved_type_params;
            checker.self_ty = saved_self_ty;
            checker.bundled_stdlib_declaration = saved_bundled;
            if let Some(info) = checker.structs.get_mut(name) {
                info.fields = field_types?;
            }
        }

        Ok(Self { checker })
    }

    pub fn require(&self, ty: &Ty, trait_name: &str) -> Result<(), ConformanceFailure> {
        if self.checker.conforms_to(ty, trait_name) {
            Ok(())
        } else {
            Err(ConformanceFailure {
                reason: self.checker.trait_failure_reason(ty, trait_name),
            })
        }
    }

    /// Answer an `IsTrivially{Movable,Copyable,Deinitable}[T]` comptime predicate.
    pub fn trivially(&self, kind: mojito_types::types::TrivialLifecycle, ty: &Ty) -> bool {
        self.checker.is_trivially(kind, ty)
    }

    /// Struct `name` of the imported package, as the package's check
    /// resolved it; its record decodes on first lookup.
    pub fn package_struct(&self, name: &str) -> Option<PackageStruct<'_>> {
        self.names_package_struct(name)
            .then(|| self.checker.structs.get(name).map(PackageStruct))
            .flatten()
    }

    /// Whether the imported package declares struct `name`, decoding
    /// nothing.
    pub fn names_package_struct(&self, name: &str) -> bool {
        self.installed().is_some()
            && !self.checker.structs.own().contains_key(name)
            && self.checker.structs.contains_key(name)
    }

    /// Whether the imported package declares function `name`, decoding at
    /// most the lowered part of the declaration owning the name.
    pub fn names_package_function(&self, name: &str) -> bool {
        self.installed().is_some() && self.checker.overload_sets.is_function(name)
    }

    /// The compile-time parameters of the imported package's function
    /// `name`, when it is one generic declaration rather than an overload
    /// family.
    pub fn package_function_params(&self, name: &str) -> Option<&[ParamDecl]> {
        match (self.installed()?.binding(name)?.1.ty).kind() {
            TyKind::GenericFunc(generic) => Some(&generic.decls),
            _ => None,
        }
    }

    /// The compile-time member `member` of `base`, an instance of a
    /// package struct: a type member as the check projects it, a value
    /// member at the instance's arguments.
    ///
    /// # Errors
    ///
    /// `base` is no package struct, or declares no such member.
    pub fn package_associated_value(&self, base: &Ty, member: &str) -> Result<CtValue, TypeError> {
        let missing = || TypeError::NoSuchAssociatedType {
            object_type: base.to_string(),
            member: member.to_string(),
        };
        let TyKind::Struct(name, arguments) = base.kind() else {
            return Err(missing());
        };
        let info = self
            .package_struct(name)
            .ok_or_else(|| TypeError::UnknownType(name.clone()))?
            .0;
        if info.parameterized_associated.contains_key(member)
            || matches!(info.associated.get(member), Some(CtValue::Type(_)))
        {
            return self
                .checker
                .associated_type_from_base(base, member, &[])
                .map(CtValue::Type);
        }
        let value = info.associated.get(member).ok_or_else(missing)?;
        self.checker
            .struct_member_at(info, arguments, value)
            .map(ParamExpr::into_value)
    }

    /// The field types of `base`, an instance of a package struct, in
    /// declaration order, each at the instance's arguments.
    pub fn package_field_types(&self, base: &Ty) -> Option<Vec<Ty>> {
        let TyKind::Struct(name, arguments) = base.kind() else {
            return None;
        };
        let info = self.package_struct(name)?.0;
        Some(
            info.fields
                .iter()
                .map(|(_, ty)| substitute_at(ty, info, arguments))
                .collect(),
        )
    }

    fn installed(&self) -> Option<&InstalledInterfaces> {
        self.checker
            .installed
            .as_ref()
            .map(|(installed, _)| installed)
    }
}

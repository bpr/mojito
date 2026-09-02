//! The `ConformanceOracle` implementation.

use super::*;

impl ConformanceOracle {
    pub(crate) fn from_program(stmts: &[Stmt]) -> Result<Self, TypeError> {
        let mut checker = Checker::new();

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
                conformance_conditions,
                methods,
                fieldwise_init,
                ..
            } = &statement.kind
            else {
                return Ok(());
            };

            let decls = match checker.classify_params(type_params) {
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
                    fixed_arguments: None,
                    conforms: conforms.clone(),
                    callable_conformance: None,
                    callable_target: None,
                    conformance_conditions: conformance_conditions.iter().cloned().collect(),
                    fields: Vec::new(),
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
            if type_params.is_empty()
                && !matches!(
                    value.kind,
                    ExprKind::Identifier(_) | ExprKind::TypeApply { .. } | ExprKind::TypeValue(_)
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
                    if matches!(**ty, Ty::Dtype | Ty::Struct(..)))
            }) {
                // Pack-dependent fields are expanded into ordinary concrete
                // fields/types by specialization; DType-/struct-valued
                // templates fold their fields the same way. The template
                // itself cannot be resolved as a single erased type.
                continue;
            }
            let self_ty = Ty::Struct(name.clone(), decls.iter().map(param_as_arg).collect());
            let saved_self_decls = std::mem::replace(&mut checker.self_decls, decls);
            let saved_type_params =
                std::mem::replace(&mut checker.enclosing_type_params, type_params.clone());
            let saved_self_ty = checker.self_ty.replace(self_ty);
            let saved_bundled = std::mem::replace(
                &mut checker.bundled_stdlib_declaration,
                is_bundled_stdlib_source(statement.module.as_deref()),
            );
            // Best-effort associated-member lowering BEFORE field resolution,
            // so a field type may apply the struct's own comptime alias
            // (`var iter: Self.dict_entry_iter`). A body this signature-only
            // oracle cannot lower is skipped and fails closed at its use
            // site, exactly like the generic-alias registration above.
            if let Ok((associated_values, associated_constraints, parameterized)) =
                checker.check_struct_associated(associated)
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

    pub(crate) fn require(&self, ty: &Ty, trait_name: &str) -> Result<(), ConformanceFailure> {
        if self.checker.conforms_to(ty, trait_name) {
            Ok(())
        } else {
            Err(ConformanceFailure {
                reason: self.checker.trait_failure_reason(ty, trait_name),
            })
        }
    }

    /// Answer an `IsTrivially{Movable,Copyable,Deinitable}[T]` comptime predicate.
    pub(crate) fn trivially(&self, kind: crate::types::TrivialLifecycle, ty: &Ty) -> bool {
        self.checker.is_trivially(kind, ty)
    }
}

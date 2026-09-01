//! Static semantic checker: the authoritative handoff between elaborated AST and
//! compiler lowering. It resolves annotations, calls, traits, and conventions
//! into [`CheckedProgram`](crate::checked::CheckedProgram). It is a *sound*
//! approximation: if [`check`] succeeds, compiled execution will not raise
//! `UndefinedVariable`, `TypeError`, `NotCallable`, `ArityMismatch`, or
//! `ClosureEscape`. It is deliberately not *complete* — see the forward-reference
//! note below — so a few valid Mojo programs are rejected.
//!
//! ## Scoping
//! A stack of scopes (`Vec<HashMap<String, Ty>>`) models lexical name lookup.
//! Names are bound *sequentially* in source order, and a nested `def` body is checked at its definition
//! site with the enclosing scopes still on the stack (so capture is lexical).
//! One consequence: a function body may not forward-reference a sibling `def`
//! declared later in the same block (mutual recursion). Choosing soundness over completeness here keeps
//! the checker simple; hoisting `def` signatures per block is future work.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::ast::{
    ArgConvention, Dtype, Expr, ExprKind, FnParam, InfixOp, Method, PrefixOp, Stmt, StmtKind,
    StructComptime, SubscriptArg, TStringPart, TraitComptime, Type as SourceType,
};
use crate::call::{
    ArgSlot, CallVariadics, MatchError, effective_keyword_only_index, match_call_slots,
    regular_marker_index,
};
use crate::ct::{CtExpr, CtValue};
use crate::error::TypeError;
use crate::token::SourceSpan;
use crate::types::{
    CallableDefault, ConstraintOperand, DependentType, GenericConstraint, ParamDecl, SliceKind, Ty,
    TyArg, array_element, array_parts, array_type, contains_infer, dict_elements, dict_type,
    list_element, list_type, range_type, set_element, set_type, tuple_elements,
    tuple_type as nominal_tuple_type,
};

/// The checker's value-coercion predicate, shared with MIR verification so the
/// verifier never re-derives conversion rules.
pub(crate) fn value_coerces(from: &Ty, to: &Ty) -> bool {
    coerces(from, to)
}

/// A signature-qualified abstract `__call__` symbol for an indirect callable
/// contract. The VM retargets this symbol to a nominal receiver's runtime type;
/// ordinary function and closure values ignore it. Keeping the signature here
/// avoids falling back to arity when a callable struct overloads `__call__` on
/// parameter type.
pub(crate) fn callable_contract_target(ty: &Ty) -> Option<String> {
    let contract = callable_contract_ty(ty)?;
    let (params, variadic, kw_variadic) = match contract {
        Ty::Func {
            params,
            variadic,
            kw_variadic,
            ..
        } => (params, variadic, kw_variadic),
        _ => return None,
    };
    let signature_types = params.iter().chain(variadic.iter().map(Box::as_ref));
    let signature = crate::symbol::SignatureKey::from_tys(signature_types)
        .with_kw_variadic(kw_variadic.as_deref());
    Some(crate::symbol::method_symbol(
        "__trait_dispatch",
        "__call__",
        &signature,
    ))
}

/// Recover the monomorphic or generic callable contract carried either directly
/// by a function type or indirectly by a callable-bounded type parameter.
pub(crate) fn callable_contract_ty(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Func { .. } | Ty::GenericFunc { .. } => Some(ty),
        Ty::Param {
            callable_bound: Some(bound),
            ..
        } => callable_contract_ty(bound),
        _ => None,
    }
}

/// Whether a concrete monomorphic callable implementation fulfills an
/// anonymous `def(...)` trait contract. This is intentionally directional:
/// non-raising/read-only implementations may fulfill raising/mutable contracts,
/// but not vice versa. Binder constraints are directional the other way
/// (upstream 2026-08): every `where` constraint the implementation declares
/// must be declared by the contract — otherwise calls through the contract
/// could violate the implementation's precondition — while an unconstrained
/// implementation may serve a constrained contract.
pub(crate) fn callable_bound_accepts(actual: &Ty, contract: &Ty) -> bool {
    if matches!(actual, Ty::GenericFunc { .. }) || matches!(contract, Ty::GenericFunc { .. }) {
        let (Some((actual_decls, actual)), Some((contract_decls, contract))) = (
            erase_generic_callable_binders(actual),
            erase_generic_callable_binders(contract),
        ) else {
            return false;
        };
        let strip = |decl: &ParamDecl| {
            let mut decl = decl.clone();
            match &mut decl {
                ParamDecl::Type { constraints, .. } | ParamDecl::Value { constraints, .. } => {
                    constraints.clear()
                }
            }
            decl
        };
        let constraints_of = |decl: &ParamDecl| -> Vec<GenericConstraint> {
            match decl {
                ParamDecl::Type { constraints, .. } | ParamDecl::Value { constraints, .. } => {
                    constraints.clone()
                }
            }
        };
        let structural = actual_decls.len() == contract_decls.len()
            && actual_decls
                .iter()
                .zip(&contract_decls)
                .all(|(actual, contract)| strip(actual) == strip(contract));
        let constraints_declared =
            actual_decls
                .iter()
                .zip(&contract_decls)
                .all(|(actual, contract)| {
                    let declared = constraints_of(contract);
                    constraints_of(actual)
                        .iter()
                        .all(|constraint| declared.contains(constraint))
                });
        return structural && constraints_declared && callable_bound_accepts(&actual, &contract);
    }

    let (
        Ty::Func {
            environment: actual_environment,
            params: actual_params,
            ret: actual_ret,
            required: actual_required,
            variadic: actual_variadic,
            kw_variadic: actual_kw_variadic,
            positional_only: actual_positional_only,
            keyword_only: actual_keyword_only,
            raises: actual_raises,
            error: actual_error,
            conventions: actual_conventions,
            ref_params: actual_ref_params,
            ref_return: actual_ref_return,
            ..
        },
        Ty::Func {
            environment: contract_environment,
            params: contract_params,
            ret: contract_ret,
            required: contract_required,
            variadic: contract_variadic,
            kw_variadic: contract_kw_variadic,
            positional_only: contract_positional_only,
            keyword_only: contract_keyword_only,
            raises: contract_raises,
            error: contract_error,
            conventions: contract_conventions,
            ref_params: contract_ref_params,
            ref_return: contract_ref_return,
            ..
        },
    ) = (actual, contract)
    else {
        return false;
    };

    callable_environment_coerces(actual_environment, contract_environment)
        && actual_params.len() == contract_params.len()
        && actual_params
            .iter()
            .zip(contract_params)
            .all(|(actual, contract)| actual == contract)
        && coerces(actual_ret, contract_ret)
        && actual_required.len() == contract_required.len()
        && actual_required
            .iter()
            .zip(contract_required)
            .all(|(actual, contract)| !*actual || *contract)
        && actual_variadic.is_none()
        && contract_variadic.is_none()
        && actual_kw_variadic.is_none()
        && contract_kw_variadic.is_none()
        && actual_positional_only == contract_positional_only
        && actual_keyword_only == contract_keyword_only
        && actual_conventions.len() == contract_conventions.len()
        && actual_conventions
            .iter()
            .zip(contract_conventions)
            .all(|(actual, contract)| callable_convention_accepts(*actual, *contract))
        && actual_ref_params == contract_ref_params
        && actual_ref_return == contract_ref_return
        && (!*actual_raises || *contract_raises)
        && match (actual_error.as_deref(), contract_error.as_deref()) {
            (None, _) | (Some(Ty::Never), _) => true,
            (Some(_), None) => false,
            (Some(actual), Some(Ty::Error)) => actual != &Ty::Never,
            (Some(actual), Some(contract)) => actual == contract,
        }
}

/// Type-check a whole program. Convenience wrapper over [`Checker`].
pub fn check(stmts: &[Stmt]) -> Result<(), TypeError> {
    check_program(stmts).map(|_| ())
}

/// Type-check and retain the semantic facts consumed by lowering/backends.
pub fn check_program(stmts: &[Stmt]) -> Result<crate::checked::CheckedProgram, TypeError> {
    check_program_with_materialized_callables(stmts, HashMap::new())
}

/// Check compiler-generated Tuple declarations with the exact callable types
/// referenced by their opaque, parser-unconstructible annotation ids.
pub(crate) fn check_program_with_materialized_callables(
    stmts: &[Stmt],
    materialized_callables: HashMap<String, Ty>,
) -> Result<crate::checked::CheckedProgram, TypeError> {
    let mut expanded = expand_trait_defaults(stmts)?;
    // Source locations survive elaboration clones and therefore cannot identify
    // semantic occurrences. Re-key the final checked tree after the last
    // checker-side cloning transform, before any fact table is populated.
    crate::ast::rekey_syntax(&mut expanded);
    // Two-phase transfer effects: a call site checked before its callee's
    // body only sees effects already committed, so the check reruns — seeded
    // with the prior round's committed map — whenever some call site
    // observed a stale (since-grown) callee entry. Effects grow
    // monotonically over a finite lattice, so the fixpoint is small; the cap
    // guards checker defects, not user programs.
    const TRANSFER_EFFECT_ROUNDS: usize = 4;
    let mut transfer_seed: HashMap<String, Vec<crate::checked::TransferEffect>> = HashMap::new();
    let mut call_through_seed: HashMap<String, Vec<crate::checked::CallThroughEffect>> =
        HashMap::new();
    let mut rounds = 0;
    let checker = loop {
        let mut checker = Checker::new_with_materialized_callables(
            materialized_callables.clone(),
            std::mem::take(&mut transfer_seed),
            std::mem::take(&mut call_through_seed),
        );
        checker.check_program(&expanded)?;
        checker.check_reference_result_reads()?;
        fn first_stale<E: PartialEq + Clone>(
            committed: &HashMap<String, Vec<E>>,
            observations: &HashMap<String, Vec<E>>,
        ) -> Option<String> {
            observations
                .iter()
                .find(|(name, seen)| {
                    let now = committed.get(*name).cloned().unwrap_or_default();
                    now.len() != seen.len() || now.iter().any(|effect| !seen.contains(effect))
                })
                .map(|(name, _)| name.clone())
        }
        let stale = first_stale(
            &checker.transfer_effects.borrow(),
            &checker.effect_observations.borrow(),
        )
        .or_else(|| {
            first_stale(
                &checker.call_through_effects.borrow(),
                &checker.call_through_observations.borrow(),
            )
        });
        let Some(callable) = stale else {
            break checker;
        };
        rounds += 1;
        if rounds > TRANSFER_EFFECT_ROUNDS {
            return Err(TypeError::TransferEffectDivergence {
                rounds: TRANSFER_EFFECT_ROUNDS,
                callable,
            });
        }
        transfer_seed = checker.transfer_effects.borrow().clone();
        call_through_seed = checker.call_through_effects.borrow().clone();
    };
    let explicit_destroy_types = checker
        .structs
        .iter()
        .filter_map(|(name, info)| {
            let self_ty = Ty::Struct(name.clone(), info.decls.iter().map(param_as_arg).collect());
            (!checker.is_deinitable(&self_ty)).then(|| {
                (
                    name.clone(),
                    crate::checked::ExplicitDestroyInfo {
                        message: info.explicit_destroy_message.clone().unwrap_or_else(|| {
                            "value is not implicitly deletable and must be explicitly destroyed"
                                .to_string()
                        }),
                        destructors: info.explicit_destructors.clone(),
                        fields: info
                            .fields
                            .iter()
                            .filter_map(|(field, ty)| match ty {
                                Ty::Struct(field_ty, _) if !checker.is_deinitable(ty) => {
                                    Some((field.clone(), field_ty.clone()))
                                }
                                _ => None,
                            })
                            .collect(),
                    },
                )
            })
        })
        .collect();
    {
        let binding_types = checker.binding_types.borrow();
        let comprehension_bindings = checker.comprehension_bindings.borrow();
        let deletability = checker.explicit_destroy_deletability.borrow();
        crate::explicit_destroy::check(
            &expanded,
            &binding_types,
            &comprehension_bindings,
            &deletability,
            &explicit_destroy_types,
        )?;
    }
    Ok(crate::checked::CheckedProgram::new(
        expanded,
        checker.overload_targets.into_inner(),
        checker.contextual_bases.into_inner(),
        checker.generic_instantiations.into_inner(),
        checker.call_transfers.into_inner(),
        checker.implicit_conversions.into_inner(),
        checker.conversion_source_borrows.into_inner(),
        checker.declaration_types.into_inner(),
        checker.generic_parameters.into_inner(),
        checker.expression_types.into_inner(),
        checker.expression_bindings.into_inner(),
        checker.statement_bindings.into_inner(),
        checker.declaration_captures.into_inner(),
        checker.comprehension_bindings.into_inner(),
        checker.expression_place_types.into_inner(),
        checker.binding_types.into_inner(),
        checker.expression_effects.into_inner(),
        checker.selected_calls.into_inner(),
        checker.subscript_descriptors.into_inner(),
        checker.iteration_protocols.into_inner(),
        checker.simd_constructions.into_inner(),
        checker.operation_adjustments.into_inner(),
        checker.tuple_unpack_plans.into_inner(),
        checker.interior_references.into_inner(),
        checker.interior_invalidations.into_inner(),
        explicit_destroy_types,
        checker.explicit_destroy_calls.into_inner(),
        checker.reference_value_uses.into_inner(),
        checker.copy_place_value_uses.into_inner(),
        checker.call_place_uses.into_inner(),
        checker.borrowed_read_call_places.into_inner(),
        checker.implicitly_copied_consuming_receivers.into_inner(),
        checker.declaration_effects.into_inner(),
    ))
}

/// A declaration-only view of the checker's conformance registry for phases
/// that necessarily run before whole-program type checking.  Compile-time
/// specialization uses this to validate an inferred heterogeneous type pack at
/// its call site; it must not grow a second, subtly different implementation of
/// trait conformance.
///
/// The oracle records trait refinement, nominal struct conformances,
/// conformance conditions, field types, and lifecycle method presence.  Method
/// bodies and full requirement signatures remain the ordinary checker's job
/// after elaboration.
pub(crate) struct ConformanceOracle {
    checker: Checker,
}

/// Evidence retained when a pre-check conformance query fails.
pub(crate) struct ConformanceFailure {
    pub(crate) reason: Option<String>,
}

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

/// Type-check a program and return the concrete lowered callee chosen for every
/// overloaded call site. MIR lowering uses this side table so source calls like
/// `f(x)` can lower to a signature-specific function even when overloads share
/// the same arity.
pub fn resolve_overload_targets(stmts: &[Stmt]) -> Result<HashMap<SourceSpan, String>, TypeError> {
    Ok(check_program(stmts)?.overload_targets().clone())
}

/// A static type checker over the parsed AST. Top-level struct and trait
/// declarations register order-independently (shells, member types, method
/// signatures) before the source-order walk checks conformance and bodies;
/// everything else checks in a single source-order pass.
pub struct Checker {
    /// Lexical scope chain, innermost last. Starts with the global scope.
    scopes: Vec<HashMap<String, Ty>>,
    /// Binding mutability, parallel to `scopes`. `var` locals are writable;
    /// ordinary function parameters are not.
    mutable_scopes: Vec<HashMap<String, bool>>,
    /// Stable identities for value bindings, parallel to `scopes`. Origin and
    /// loan facts use these identities so a shadowing declaration cannot be
    /// confused with the binding of the same source name in an outer scope.
    owner_scopes: Vec<HashMap<String, crate::origin::OwnerId>>,
    /// Origins retained inside reference-bearing aggregate bindings, parallel
    /// to the lexical value scopes.  Unlike `Ty::Struct`, this preserves the
    /// use-site owner identity needed for escape checking.
    aggregate_origin_scopes: Vec<HashMap<String, Vec<crate::origin::Origin>>>,
    /// Field-specific projection of `aggregate_origin_scopes`. Keeping direct
    /// reference fields separate prevents a write through one stored handle
    /// from invalidating interiors reached through an unrelated field.
    aggregate_field_origin_scopes:
        Vec<HashMap<String, HashMap<String, Vec<crate::origin::Origin>>>>,
    /// Reference-parameter handle types. Parameter expression typing still
    /// reads through to the declared referent, while storage contexts can ask
    /// for the handle explicitly.
    reference_parameter_scopes: Vec<HashMap<String, crate::origin::RefTy>>,
    /// The enclosing struct's origin binder a `ref[Self.o]` parameter's clause
    /// names, by the parameter's owner: `Pointer(to=param)` mints that binder
    /// (upstream's iterator-storage shape `self.src = Pointer(to=xs)` into a
    /// `Pointer[T, Self.o]` field) instead of the parameter slot's place.
    reference_parameter_binders: HashMap<crate::origin::OwnerId, crate::origin::PointerOrigin>,
    /// Origin-parameter declarations for callable values, parallel to the
    /// lexical value scopes. The outer vector stored per name has one entry per
    /// overload declaration. Each entry also retains the original compile-time
    /// parameter order so erased Origin arguments can participate in overload
    /// and generic candidate selection.
    callable_origin_scopes: Vec<HashMap<String, Vec<CallableOriginSignature>>>,
    /// Monotonic binding-identity source. A `Cell` because materialized
    /// borrow-source temporaries mint anonymous owners from shared-borrow
    /// inference contexts.
    next_owner: std::cell::Cell<u32>,
    /// How many leading entries of `enclosing_type_params` are the enclosing
    /// STRUCT's own parameters (method-own parameters are appended after
    /// them while a method checks). Member origin clauses reject bare
    /// references into this prefix; later (method-own) binders stay bare.
    enclosing_struct_type_params: std::cell::Cell<usize>,
    /// True while resolving a signature (return) annotation: an origin
    /// argument that is syntactically origin-shaped but names places only the
    /// body can resolve (`origin_of(self.entries)`) is accepted and erased
    /// instead of failing place resolution.
    signature_origin_leniency: std::cell::Cell<bool>,
    /// Index of the local scope for each function currently being checked.
    function_bases: Vec<usize>,
    /// Function/method scope base and caller-owned inputs which may legally
    /// appear inside a returned reference-bearing aggregate.
    aggregate_escape_contexts: Vec<(usize, HashSet<crate::origin::OwnerId>)>,
    /// Explicit capture policy for each nested function body being checked.
    capture_contexts: RefCell<Vec<CapturePolicy>>,
    /// Defined structs, by name (a separate namespace from value bindings).
    structs: HashMap<String, StructInfo>,
    /// Top-level struct symbols in the checked program, collected before body
    /// checking. Concrete Tuple types can therefore select their generated
    /// implementation independent of declaration order.
    declared_structs: HashSet<String>,
    /// Top-level structs whose shell, member types, and method signatures were
    /// registered by `check_program`'s order-independent pre-passes. The
    /// source-order walk removes each entry and runs only the completion phase
    /// (conformance verification and method bodies).
    predeclared_structs: HashSet<String>,
    /// Top-level traits registered by `check_program`'s pre-pass; the walk
    /// removes each entry instead of re-registering.
    predeclared_traits: HashSet<String>,
    /// Fixed semantic arguments for every compiler-generated public Tuple in
    /// the final program. This signature-only predeclaration is populated as a
    /// closed set before sequential member checking, so reciprocal transforms
    /// can name each other's complete nominal type without exposing ordinary
    /// source forward references.
    predeclared_generated_tuple_arguments: HashMap<String, Vec<TyArg>>,
    /// Exact semantic callable contracts named by compiler-only opaque type
    /// ids in generated Tuple declarations. Parsed source cannot populate this
    /// namespace.
    materialized_callables: HashMap<String, Ty>,
    /// Generated Tuple implementations are emitted at the stdlib template
    /// position, which may precede user element declarations. While checking
    /// that compiler-owned specialization only, permit nominal forward type
    /// identities; ordinary source declarations retain sequential visibility.
    allow_generated_tuple_forward_types: bool,
    /// Defined traits, by name (their method requirements).
    traits: HashMap<String, TraitInfo>,
    /// Stack of a generic `def`'s checked type parameters, innermost last. A
    /// bare `T` annotation resolves to the complete `Ty::Param`, including any
    /// anonymous callable-trait contract.
    /// (A `def`'s *value* parameters are ordinary `Int` locals, not here.)
    tparams: Vec<HashMap<String, Ty>>,
    /// The enclosing struct's parameters while checking its fields and methods,
    /// so `Self.T` resolves to `Ty::Param` and `Self.n` to a value parameter.
    /// Saved/restored around a (possibly nested) struct definition.
    self_decls: Vec<ParamDecl>,
    /// Positive conformance facts guaranteed by the active method's `where`
    /// clause. These are deliberately kept separate from `self_decls`: adding
    /// a temporary bound to the declaration changes the identity of `Ty::Param`
    /// and can make an otherwise identical return type fail to match. The
    /// facts refine capability queries only while that method body is checked.
    assumed_conformances: Vec<HashSet<(String, String)>>,
    enclosing_type_params: Vec<crate::ast::TypeParam>,
    /// The `Ty` denoted by a bare `Self` while checking a struct's members (the
    /// struct type) or a trait's requirements (`Ty::SelfType`). `None` elsewhere.
    self_ty: Option<Ty>,
    /// Trait-associated comptime requirements in scope while checking a trait's
    /// own method requirement signatures, so `Self.Element` can resolve.
    trait_self_comptime: Vec<HashMap<String, CtMemberReq>>,
    /// Exact integer constants declared by `comptime NAME = value`.
    comptimes: HashMap<String, crate::literal::IntLiteral>,
    /// Generic top-level type aliases declared by `comptime NAME[params] = Type`,
    /// expanded per application during type resolution. See [`ComptimeAlias`].
    comptime_aliases: HashMap<String, ComptimeAlias>,
    /// Whether `self` is writable in the method body being checked — set while
    /// checking a `mut self` method's body (so `self.x = e` is allowed there).
    self_mutable: bool,
    /// An `out self` lifecycle initializer is establishing field storage.  For
    /// a reference-valued field, assigning a reference here stores its handle;
    /// later assignments write through the established handle instead.
    self_initializing: bool,
    /// Per-method accumulation of symbolic origin write requirements inherited
    /// from calls through generic receiver fields. The completed frame is
    /// merged into that method's registered signature.
    parametric_write_frames: RefCell<Vec<Vec<crate::origin::OriginParamId>>>,
    /// The declaration currently being checked comes from the bundled
    /// standard-library crossing module (`stdlib/std/memory.mojo`). Only such
    /// declarations may name compiler-private storage types (`__UninitStorage`)
    /// in sourceless type-annotation positions.
    bundled_stdlib_declaration: bool,
    /// Source-span to lowered callee for calls whose source name denotes an
    /// overload set. Interior mutability keeps expression inference usable from
    /// read-only helper methods while still recording resolution facts.
    overload_targets: RefCell<HashMap<SourceSpan, String>>,
    /// Checker-resolved base type name per `$contextual` leading-dot sentinel
    /// (keyed by the sentinel identifier's span); HIR substitutes the name.
    contextual_bases: RefCell<HashMap<SourceSpan, String>>,
    /// The resolved generic application per bound-generic call site (callee +
    /// exact compile-time arguments), retained for instantiation discovery.
    generic_instantiations: RefCell<HashMap<SourceSpan, crate::checked::GenericInstantiation>>,
    /// Per-body accumulation frames for inferred loan-transfer effects.
    transfer_frames: RefCell<Vec<TransferFrame>>,
    /// Inferred per-callable transfer effects, keyed by callable name
    /// (`name` / `Struct.method`); consulted at later call sites.
    transfer_effects: RefCell<HashMap<String, Vec<crate::checked::TransferEffect>>>,
    /// Caller-substituted transfers per call occurrence, handed to MIR.
    call_transfers: RefCell<HashMap<SourceSpan, Vec<crate::checked::CheckedCallTransfer>>>,
    /// First-seen callee effects per `apply_transfer_effects` lookup. The
    /// two-phase pass reruns the check when a callee's final committed
    /// effects differ from what its stalest call-site query observed.
    effect_observations: RefCell<HashMap<String, Vec<crate::checked::TransferEffect>>>,
    /// Inferred higher-order call-through residues per callable, keyed like
    /// `transfer_effects`; each call site resolves them against the concrete
    /// callable it supplies.
    call_through_effects: RefCell<HashMap<String, Vec<crate::checked::CallThroughEffect>>>,
    /// First-seen call-through observations, mirroring `effect_observations`.
    call_through_observations: RefCell<HashMap<String, Vec<crate::checked::CallThroughEffect>>>,
    /// Origins transferred into a binding by a callee's store (keyed by the
    /// binding's owner) — an interior-mutability overlay over the
    /// aggregate-origin scopes, merged on lookup.
    transferred_origins: RefCell<HashMap<crate::origin::OwnerId, Vec<crate::origin::Origin>>>,
    /// Innermost-last observation frames recording whether a raising
    /// operation that escapes the bracketed region was checked inside it.
    /// Owned iteration over linear elements pushes one around its body —
    /// such a call aborts the loop and abandons residual explicit-destroy
    /// obligations — and every callable body pushes a barrier frame so a
    /// nested `def`'s raising calls never mark an enclosing loop. The
    /// `usize` is the `handled_raise_depth` at region entry: a call under a
    /// deeper depth is contained by a `try` inside the region and does not
    /// escape it, while a handler outside the region still lets the error
    /// abort the region itself.
    raise_observation_frames: RefCell<Vec<(usize, bool)>>,
    implicit_conversions: RefCell<HashMap<SourceSpan, String>>,
    /// Sites in `implicit_conversions` whose selected constructor borrows its
    /// single argument through a `ref [origin]` parameter: the conversion
    /// result borrows the source place (temporary-origin inference). The
    /// value is the loan mutability, solved like the explicit construction
    /// path's `BorrowRefArguments`.
    conversion_source_borrows: RefCell<HashMap<SourceSpan, bool>>,
    simd_constructions: RefCell<HashMap<SourceSpan, (Dtype, i64)>>,
    /// Checked operation decisions — `Variant` construction/tag/projection/
    /// update and origin-bearing pointer construction — keyed by the source
    /// expression.  These cross the typed boundary so MIR never reinterprets
    /// syntax.
    operation_adjustments: RefCell<HashMap<SourceSpan, crate::checked::SemanticAdjustment>>,
    /// Whether type resolution is inside a storage annotation (a struct field
    /// or local `var` type) — the positions where explicit origin slots must
    /// be bound. An initialized local may still leave a BARE generic to infer
    /// wholly from its initializer (upstream-attested via `StringSlice`);
    /// fields and uninitialized locals may not.
    strict_storage_annotation: std::cell::Cell<StorageStrictness>,
    /// Synthetic tuple element reads introduced by unpacking have no source
    /// expression nodes. Retain their checked types and exact generated
    /// accessors on the RHS expression for HIR/MIR lowering.
    tuple_unpack_plans:
        RefCell<HashMap<SourceSpan, Vec<crate::checked::CheckedTupleUnpackElement>>>,
    /// Place expressions that define a fresh interior-reference generation.
    /// Kept separate from operation adjustments because a Variant projection,
    /// for example, carries both facts at the same checked node.
    interior_references: RefCell<HashMap<SourceSpan, crate::origin::OriginPlace>>,
    /// Mutations which invalidate interior generations below checked bases.
    interior_invalidations: RefCell<HashMap<SourceSpan, Vec<crate::checked::InteriorInvalidation>>>,
    declaration_types: RefCell<HashMap<crate::checked::AnnotationSite, Ty>>,
    generic_parameters: RefCell<HashMap<crate::checked::GenericSite, Vec<crate::types::ParamDecl>>>,
    /// Checked raising contract and reference-return fact per callable
    /// declaration; lowering never re-reads source `raises`/return syntax.
    declaration_effects:
        RefCell<HashMap<crate::checked::AnnotationSite, crate::checked::DeclarationEffect>>,
    expression_types: RefCell<HashMap<SourceSpan, Ty>>,
    expression_bindings: RefCell<HashMap<SourceSpan, crate::origin::OwnerId>>,
    /// Stable identities assigned by declarations and other binding statements.
    /// HIR uses these facts to map checked owners to runtime slots without
    /// recovering a binding from its source spelling.
    statement_bindings: RefCell<HashMap<SourceSpan, crate::origin::OwnerId>>,
    /// Explicit capture entries resolved at the nested declaration site. Keeping
    /// unused entries is essential: a move capture still transfers at declaration.
    declaration_captures: RefCell<HashMap<SourceSpan, Vec<crate::checked::CheckedCapture>>>,
    /// Stable identities/types for the lexical binders introduced by each
    /// comprehension, retained for checked HIR and explicit-destroy analysis.
    comprehension_bindings:
        RefCell<HashMap<SourceSpan, Vec<crate::checked::CheckedComprehensionBinding>>>,
    expression_place_types: RefCell<HashMap<SourceSpan, Ty>>,
    binding_types: RefCell<HashMap<SourceSpan, Ty>>,
    /// Positive site-sensitive drop facts retained for the later explicit-
    /// destroy CFG pass. Conditional conformances are meaningful only in the
    /// constraint environment in which a binding was checked.
    explicit_destroy_deletability: RefCell<crate::explicit_destroy::CheckedDeletability>,
    /// Selected call effects keyed by the checked call expression. This records
    /// the contract chosen during overload/bounded dispatch so later phases do
    /// not have to rediscover it from source syntax.
    expression_effects: RefCell<HashMap<SourceSpan, crate::checked::EffectFacts>>,
    /// Complete overload/origin/effect contract for a selected method-like
    /// call.  Nominal subscripts and ordinary method syntax share this fact.
    selected_calls: RefCell<HashMap<SourceSpan, crate::checked::CheckedCallContract>>,
    /// Subscript descriptor construction is orthogonal to call selection and
    /// may coexist with a reference-result adjustment at the same expression.
    subscript_descriptors: RefCell<HashMap<SourceSpan, SubscriptDescriptorPlan>>,
    /// Exact iterator protocol selected for each loop/comprehension iterable.
    /// Lowering consumes this fact instead of re-selecting `__iter__` by name.
    iteration_protocols: RefCell<HashMap<SourceSpan, crate::checked::IterationProtocol>>,
    explicit_destroy_calls: RefCell<std::collections::HashSet<SourceSpan>>,
    /// Expressions whose reference handle, rather than referent value, is
    /// required by a reference binding or origin-bearing aggregate operation.
    /// The bool records whether the resulting capability is writable.
    reference_value_uses: RefCell<HashMap<SourceSpan, bool>>,
    /// Reference-result reads proven copyable in their site-sensitive generic
    /// constraint environment. Final validation runs after those method scopes
    /// have been popped, so it must retain rather than recompute this fact.
    copyable_reference_result_reads: RefCell<HashSet<SourceSpan>>,
    /// Reference-result expressions used as method-call receivers. The call
    /// borrows the referent for its `self` convention (consuming receivers
    /// are gated separately), so the result is never read out as a value.
    borrowed_reference_receivers: RefCell<HashSet<SourceSpan>>,
    /// Place expressions selected for an independent value copy at a consuming
    /// boundary. This stays checker-owned because conditional Copyable
    /// conformance can depend on the active generic constraint environment.
    copy_place_value_uses: RefCell<HashSet<SourceSpan>>,
    /// Actual arguments whose caller place must remain live through a selected
    /// `mut`/`ref` call. This checker-owned fact keeps MIR lowering from
    /// retaining ordinary copied arguments merely because they are syntactic
    /// places.
    call_place_uses: RefCell<HashSet<SourceSpan>>,
    /// Read-convention place arguments the selected call may bind by borrow
    /// instead of the implicit `__copyinit__` read: shared reads whose place no
    /// exclusive access overlaps within the call. Checker-owned because the
    /// effective conventions and within-call exclusivity are resolved here.
    borrowed_read_call_places: RefCell<HashSet<SourceSpan>>,
    /// Consuming method calls whose place receiver is implicitly copied. Kept
    /// separate from the single operation-adjustment slot so parameterized
    /// method metadata can coexist at the same expression.
    implicitly_copied_consuming_receivers: RefCell<HashSet<SourceSpan>>,
    return_ref_contracts: Vec<Option<ReturnRefContract>>,
    named_result_context: Vec<bool>,
    raising_context: Vec<Option<Ty>>,
    handled_raise_depth: usize,
    handled_raise_types: RefCell<Vec<Vec<Ty>>>,
    uninitialized: RefCell<HashSet<crate::origin::OwnerId>>,
}

impl Checker {
    pub fn new() -> Self {
        Self::new_with_materialized_callables(HashMap::new(), HashMap::new(), HashMap::new())
    }

    fn new_with_materialized_callables(
        materialized_callables: HashMap<String, Ty>,
        transfer_seed: HashMap<String, Vec<crate::checked::TransferEffect>>,
        call_through_seed: HashMap<String, Vec<crate::checked::CallThroughEffect>>,
    ) -> Self {
        // The bundled seeds are never inferred from bodies, so a prior
        // round's committed map overlays them rather than replacing them.
        let mut transfer_effects = seeded_transfer_effects();
        for (callable, effects) in transfer_seed {
            let entry = transfer_effects.entry(callable).or_default();
            for effect in effects {
                if !entry.contains(&effect) {
                    entry.push(effect);
                }
            }
        }
        Self {
            scopes: vec![HashMap::new()],
            mutable_scopes: vec![HashMap::new()],
            owner_scopes: vec![HashMap::new()],
            aggregate_origin_scopes: vec![HashMap::new()],
            aggregate_field_origin_scopes: vec![HashMap::new()],
            reference_parameter_scopes: vec![HashMap::new()],
            reference_parameter_binders: HashMap::new(),
            callable_origin_scopes: vec![HashMap::new()],
            next_owner: std::cell::Cell::new(0),
            signature_origin_leniency: std::cell::Cell::new(false),
            enclosing_struct_type_params: std::cell::Cell::new(0),
            function_bases: Vec::new(),
            aggregate_escape_contexts: Vec::new(),
            capture_contexts: RefCell::new(Vec::new()),
            structs: HashMap::new(),
            declared_structs: HashSet::new(),
            predeclared_structs: HashSet::new(),
            predeclared_traits: HashSet::new(),
            predeclared_generated_tuple_arguments: HashMap::new(),
            materialized_callables,
            allow_generated_tuple_forward_types: false,
            traits: HashMap::new(),
            tparams: Vec::new(),
            self_decls: Vec::new(),
            assumed_conformances: Vec::new(),
            enclosing_type_params: Vec::new(),
            self_ty: None,
            trait_self_comptime: Vec::new(),
            comptimes: HashMap::new(),
            comptime_aliases: HashMap::new(),
            self_mutable: false,
            self_initializing: false,
            parametric_write_frames: RefCell::new(Vec::new()),
            bundled_stdlib_declaration: false,
            overload_targets: RefCell::new(HashMap::new()),
            contextual_bases: RefCell::new(HashMap::new()),
            generic_instantiations: RefCell::new(HashMap::new()),
            transfer_frames: RefCell::new(Vec::new()),
            transfer_effects: RefCell::new(transfer_effects),
            call_transfers: RefCell::new(HashMap::new()),
            effect_observations: RefCell::new(HashMap::new()),
            call_through_effects: RefCell::new(call_through_seed),
            call_through_observations: RefCell::new(HashMap::new()),
            transferred_origins: RefCell::new(HashMap::new()),
            raise_observation_frames: RefCell::new(Vec::new()),
            implicit_conversions: RefCell::new(HashMap::new()),
            conversion_source_borrows: RefCell::new(HashMap::new()),
            simd_constructions: RefCell::new(HashMap::new()),
            operation_adjustments: RefCell::new(HashMap::new()),
            strict_storage_annotation: std::cell::Cell::new(StorageStrictness::Off),
            tuple_unpack_plans: RefCell::new(HashMap::new()),
            interior_references: RefCell::new(HashMap::new()),
            interior_invalidations: RefCell::new(HashMap::new()),
            declaration_types: RefCell::new(HashMap::new()),
            generic_parameters: RefCell::new(HashMap::new()),
            declaration_effects: RefCell::new(HashMap::new()),
            expression_types: RefCell::new(HashMap::new()),
            expression_bindings: RefCell::new(HashMap::new()),
            statement_bindings: RefCell::new(HashMap::new()),
            declaration_captures: RefCell::new(HashMap::new()),
            comprehension_bindings: RefCell::new(HashMap::new()),
            expression_place_types: RefCell::new(HashMap::new()),
            binding_types: RefCell::new(HashMap::new()),
            explicit_destroy_deletability: RefCell::new(
                crate::explicit_destroy::CheckedDeletability::default(),
            ),
            expression_effects: RefCell::new(HashMap::new()),
            selected_calls: RefCell::new(HashMap::new()),
            subscript_descriptors: RefCell::new(HashMap::new()),
            iteration_protocols: RefCell::new(HashMap::new()),
            explicit_destroy_calls: RefCell::new(std::collections::HashSet::new()),
            reference_value_uses: RefCell::new(HashMap::new()),
            copyable_reference_result_reads: RefCell::new(HashSet::new()),
            borrowed_reference_receivers: RefCell::new(HashSet::new()),
            copy_place_value_uses: RefCell::new(HashSet::new()),
            call_place_uses: RefCell::new(HashSet::new()),
            borrowed_read_call_places: RefCell::new(HashSet::new()),
            implicitly_copied_consuming_receivers: RefCell::new(HashSet::new()),
            return_ref_contracts: Vec::new(),
            named_result_context: Vec::new(),
            raising_context: Vec::new(),
            handled_raise_depth: 0,
            handled_raise_types: RefCell::new(Vec::new()),
            uninitialized: RefCell::new(HashSet::new()),
        }
    }

    fn raising_allowed(&self) -> bool {
        self.handled_raise_depth > 0
            || self
                .raising_context
                .last()
                .is_some_and(|error| error.as_ref().is_some_and(|ty| *ty != Ty::Never))
    }

    fn require_error(&self, operation: impl Into<String>, error: Ty) -> Result<(), TypeError> {
        // A raising operation escapes the innermost observed region (a
        // linear owned-iteration body) unless a `try` INSIDE that region
        // contains it — a handler outside the region still aborts the region.
        if let Some((baseline, flag)) = self.raise_observation_frames.borrow_mut().last_mut()
            && self.handled_raise_depth <= *baseline
        {
            *flag = true;
        }
        if self.handled_raise_depth > 0 {
            if let Some(types) = self.handled_raise_types.borrow_mut().last_mut() {
                types.push(error);
            }
            return Ok(());
        }
        if let Some(Some(expected)) = self.raising_context.last() {
            if *expected == error {
                return Ok(());
            }
            return Err(TypeError::RaiseTypeMismatch {
                expected: expected.to_string(),
                found: error.to_string(),
            });
        }
        if self.raising_allowed() {
            Ok(())
        } else {
            Err(TypeError::UnhandledRaise(operation.into()))
        }
    }

    fn record_call_effect(&self, span: SourceSpan, error: Ty) {
        self.expression_effects.borrow_mut().insert(
            span,
            crate::checked::EffectFacts {
                raises: Some(error),
                may_suspend: false,
                diverges: false,
            },
        );
    }

    fn concrete_callable_captures(&self, ty: &Ty) -> Vec<crate::origin::CaptureOrigin> {
        use crate::origin::{CallableEnvironment, CaptureOriginSet};
        let callable = match ty {
            Ty::Struct(name, _) => self
                .structs
                .get(name)
                .and_then(|info| info.callable_conformance.as_ref()),
            other => Some(other),
        };
        match callable {
            Some(Ty::Func {
                environment: CallableEnvironment::Capturing(CaptureOriginSet::Concrete(captures)),
                ..
            })
            | Some(Ty::GenericFunc {
                environment: CallableEnvironment::Capturing(CaptureOriginSet::Concrete(captures)),
                ..
            }) => captures.clone(),
            _ => Vec::new(),
        }
    }

    /// Retain every concrete environment access that may happen while a call is
    /// active. Callable arguments are included because Mojito supports only
    /// non-escaping downward funargs: their environments may be invoked by the
    /// callee before it returns.
    fn call_capture_effects<'a>(
        &self,
        types: impl IntoIterator<Item = &'a Ty>,
    ) -> Vec<crate::origin::CaptureOrigin> {
        use crate::origin::CaptureOriginSet;
        let captures = types
            .into_iter()
            .flat_map(|ty| self.concrete_callable_captures(ty))
            .collect::<Vec<_>>();
        let CaptureOriginSet::Concrete(captures) = CaptureOriginSet::concrete(captures) else {
            unreachable!("concrete capture canonicalization stays concrete")
        };
        captures
    }

    fn record_call_capture_effects<'a>(
        &self,
        span: SourceSpan,
        types: impl IntoIterator<Item = &'a Ty>,
    ) -> Vec<crate::origin::CaptureOrigin> {
        let captures = self.call_capture_effects(types);
        if captures.is_empty() {
            return captures;
        }
        self.operation_adjustments.borrow_mut().insert(
            span,
            crate::checked::SemanticAdjustment::CallableCaptureAccesses(captures.clone()),
        );
        captures
    }

    fn record_call_environment_effects(
        &self,
        span: SourceSpan,
        callable: &Ty,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<(), TypeError> {
        // Call checking may have supplied essential context to an argument
        // before capture-effect collection runs (notably an empty collection
        // display, an exact literal, or an overloaded callable value). Reuse
        // that checked type instead of independently re-inferring the syntax
        // without its selected parameter contract.
        let checked_argument_type = |expression: &Expr| {
            let checked = {
                self.expression_types
                    .borrow()
                    .get(&expression.source_span())
                    .cloned()
            };
            match checked {
                Some(ty) => Ok(ty),
                None => self.infer(expression),
            }
        };
        let mut types = Vec::with_capacity(1 + param_args.len() + args.len() + kwargs.len());
        types.push(callable.clone());
        for argument in param_args {
            let expression = match argument {
                crate::ast::ParamArg::Value(expression) => Some(expression),
                crate::ast::ParamArg::Named { value, .. } => match &**value {
                    crate::ast::ParamArg::Value(expression) => Some(expression),
                    crate::ast::ParamArg::Type(_) | crate::ast::ParamArg::Named { .. } => None,
                },
                crate::ast::ParamArg::Type(_) => None,
            };
            if let Some(expression) = expression {
                // A bare type argument (`hash[MyHasher](x)`) is a value
                // expression syntactically; binding already resolved it as a
                // type and erased it, so it carries no runtime effect.
                let erased = self
                    .operation_adjustments
                    .borrow()
                    .get(&expression.source_span())
                    .is_some_and(|adjustment| {
                        matches!(
                            adjustment,
                            crate::checked::SemanticAdjustment::EraseCompileTimeArgument
                        )
                    });
                if erased {
                    continue;
                }
                types.push(checked_argument_type(expression)?);
            }
        }
        for argument in args {
            types.push(checked_argument_type(argument)?);
        }
        for argument in kwargs {
            types.push(checked_argument_type(&argument.value)?);
        }
        self.record_call_capture_effects(span, &types);
        Ok(())
    }

    fn declared_error(
        &self,
        raises: bool,
        raises_type: Option<&SourceType>,
    ) -> Result<Option<Ty>, TypeError> {
        if !raises {
            return Ok(None);
        }
        Ok(Some(match raises_type {
            Some(error) => self.ty_from_anno(error)?,
            None => Ty::Error,
        }))
    }

    fn lower_callable_environment(
        &self,
        type_params: &[crate::ast::TypeParam],
        thin: bool,
        capturing: Option<&crate::ast::OriginSpec>,
    ) -> Result<crate::origin::CallableEnvironment, TypeError> {
        use crate::origin::{CallableEnvironment, CaptureOriginSet, CaptureSetParamId};
        if thin && capturing.is_some() {
            return Err(TypeError::Unsupported(
                "a callable type cannot be both 'thin' and 'capturing'".to_string(),
            ));
        }
        if thin {
            return Ok(CallableEnvironment::Thin);
        }
        let Some(origins) = capturing else {
            return Ok(CallableEnvironment::Default);
        };
        match origins.as_slice() {
            [] => Ok(CallableEnvironment::Capturing(CaptureOriginSet::empty())),
            [
                Expr {
                    kind: ExprKind::Identifier(name),
                    ..
                },
            ] if name == "_" => Ok(CallableEnvironment::Capturing(CaptureOriginSet::Infer)),
            [
                Expr {
                    kind: ExprKind::Identifier(name),
                    ..
                },
            ] => {
                let index = type_params
                    .iter()
                    .position(|parameter| {
                        parameter.name == *name && parameter.bounds.as_slice() == ["OriginSet"]
                    })
                    .ok_or_else(|| TypeError::UndefinedVariable(name.clone()))?;
                Ok(CallableEnvironment::Capturing(CaptureOriginSet::Param(
                    CaptureSetParamId(index as u32),
                )))
            }
            _ => Err(TypeError::Unsupported(
                "capturing[...] requires exactly one OriginSet parameter or '_'".to_string(),
            )),
        }
    }

    /// Lower a callable annotation's origin contract. Declaration contracts
    /// normally refer to their own parameters and use `lower_ref_sig`; a
    /// contextual function-value type may additionally bind `origin_of(local)`
    /// directly to the checked local owner.
    fn lower_callable_ref_sig(
        &self,
        spec: &crate::ast::OriginSpec,
        type_params: &[crate::ast::TypeParam],
        params: &[&FnParam],
    ) -> Result<crate::origin::RefSig, TypeError> {
        use crate::origin::{Mutability, RefSig, SigMutability, SigOrigin};

        if let Ok(signature) = lower_ref_sig(spec, type_params, params, 0) {
            return Ok(signature);
        }
        let mut members = Vec::new();
        let mut bound_mutability = Vec::new();
        for expression in spec {
            match &expression.kind {
                ExprKind::Call {
                    name,
                    args,
                    kwargs,
                    param_args,
                } if name == "origin_of" && kwargs.is_empty() && param_args.is_empty() => {
                    if args.is_empty() {
                        return Err(TypeError::Unsupported(
                            "origin_of requires at least one place".to_string(),
                        ));
                    }
                    for argument in args {
                        let (root, path) = place_path(argument).ok_or_else(|| {
                            TypeError::Unsupported("origin_of requires places".to_string())
                        })?;
                        if root == "self" {
                            members.push(project_sig_origin(SigOrigin::Self_, &path));
                        } else if let Some(index) =
                            params.iter().position(|parameter| parameter.name == root)
                        {
                            members.push(project_sig_origin(SigOrigin::Param(index), &path));
                        } else {
                            let actual = self.reference_actual(argument)?;
                            bound_mutability.push(actual.mutability);
                            members.push(SigOrigin::Bound(actual.origin));
                        }
                    }
                }
                _ => members.push(lower_sig_origin_expression(
                    expression,
                    type_params,
                    params,
                    0,
                )?),
            }
        }
        let origin = match members.as_slice() {
            [] => SigOrigin::Infer,
            [single] => single.clone(),
            _ => SigOrigin::union(members),
        };
        let mutability = if bound_mutability.is_empty() {
            SigMutability::Infer
        } else if bound_mutability
            .iter()
            .all(|mutability| *mutability == Mutability::Immutable)
        {
            SigMutability::Immutable
        } else {
            SigMutability::Mutable
        };
        Ok(RefSig { origin, mutability })
    }

    fn lower_callable_ref_param_sigs(
        &self,
        type_params: &[crate::ast::TypeParam],
        params: &[&FnParam],
    ) -> Result<Vec<Option<crate::origin::RefSig>>, TypeError> {
        params
            .iter()
            .map(|parameter| {
                if parameter.convention != Some(ArgConvention::Ref) {
                    return Ok(None);
                }
                match &parameter.origin {
                    Some(spec) => self
                        .lower_callable_ref_sig(spec, type_params, params)
                        .map(Some),
                    None => Ok(Some(crate::origin::RefSig {
                        origin: crate::origin::SigOrigin::Infer,
                        mutability: crate::origin::SigMutability::Infer,
                    })),
                }
            })
            .collect()
    }

    /// Contextually instantiate a generic function value when a monomorphic
    /// callable type supplies all of its type information. Runtime execution is
    /// still type-erased; this produces the checked callable view used by the
    /// binding or argument site.
    fn value_coerces(&self, from: &Ty, to: &Ty) -> bool {
        if coerces(from, to) {
            return true;
        }
        if let Ty::Param { bounds, .. } = to
            && bounds.iter().all(|bound| self.conforms_to(from, bound))
        {
            return true;
        }
        if matches!((from, to), (Ty::GenericFunc { .. }, Ty::GenericFunc { .. }))
            && callable_bound_accepts(from, to)
        {
            return true;
        }
        if matches!((from, to), (Ty::GenericFunc { .. }, Ty::GenericFunc { .. })) {
            return callable_bound_accepts(from, to);
        }
        if let Ty::Struct(name, _) = from
            && let Some(callable) = self
                .structs
                .get(name)
                .and_then(|info| info.callable_conformance.as_ref())
        {
            return coerces(callable, to);
        }
        let (
            Ty::GenericFunc {
                environment,
                decls,
                params,
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
                ..
            },
            Ty::Func {
                params: expected_params,
                ret: expected_ret,
                ..
            },
        ) = (from, to)
        else {
            return false;
        };
        let mut patterns = params.clone();
        patterns.push((**ret).clone());
        let mut actuals = expected_params.clone();
        actuals.push((**expected_ret).clone());
        let Ok((subst, _)) =
            self.resolve_use_params("<generic callable>", decls, &[], &patterns, &actuals)
        else {
            return false;
        };
        let instantiated = Ty::Func {
            environment: environment.clone(),
            params: params.iter().map(|ty| substitute(ty, &subst)).collect(),
            names: (0..params.len())
                .map(|index| format!("arg{index}"))
                .collect(),
            ret: Box::new(substitute(ret, &subst)),
            required: required.clone(),
            variadic: variadic.as_ref().map(|ty| Box::new(substitute(ty, &subst))),
            kw_variadic: kw_variadic
                .as_ref()
                .map(|ty| Box::new(substitute(ty, &subst))),
            positional_only: *positional_only,
            keyword_only: *keyword_only,
            raises: *raises,
            error: error
                .as_ref()
                .map(|error| Box::new(substitute(error, &subst))),
            conventions: conventions.clone(),
            ref_params: ref_params.clone(),
            ref_return: ref_return.clone(),
            transfers: Default::default(),
        };
        coerces(&instantiated, to)
    }

    /// Checker-selected nominal target when the concrete callable type is
    /// known, otherwise the signature-qualified abstract dispatch target for a
    /// `def(...)` value. Runtime retargeting uses the latter only when the value
    /// is a callable struct.
    fn indirect_callable_target(&self, callable: &Ty) -> Option<String> {
        if let Ty::Struct(name, _) = callable {
            return self
                .structs
                .get(name)
                .and_then(|info| info.callable_target.clone());
        }
        callable_contract_target(callable)
    }

    /// Resolve the callable contract an actual type is *declared* to provide.
    /// Structs participate only through nominal `struct S(def(...))`
    /// conformance; merely defining a shape-compatible `__call__` is not enough.
    fn declared_callable_contract(&self, actual: &Ty) -> Option<Ty> {
        match actual {
            Ty::Func { .. } => Some(actual.clone()),
            Ty::Param {
                callable_bound: Some(bound),
                ..
            } => Some((**bound).clone()),
            Ty::Struct(name, arguments) => {
                let info = self.structs.get(name)?;
                let contract = info.callable_conformance.as_ref()?;
                Some(substitute(contract, &struct_subst(&info.decls, arguments)))
            }
            // A generic function has a callable identity only for a matching
            // generic anonymous contract. `callable_bound_accepts` rejects it
            // when the required contract is monomorphic.
            Ty::GenericFunc { .. } => Some(actual.clone()),
            // An overload set has no unique callable identity.
            Ty::Overload(_) => None,
            _ => None,
        }
    }

    fn validate_callable_parameter_bounds(
        &self,
        generic_name: &str,
        decls: &[ParamDecl],
        arguments: &[TyArg],
    ) -> Result<(), TypeError> {
        let subst = struct_subst(decls, arguments);
        for (decl, argument) in decls.iter().zip(arguments) {
            let ParamDecl::Type {
                name,
                callable_bound: Some(bound),
                ..
            } = decl
            else {
                continue;
            };
            let TyArg::Ty(actual) = argument else {
                return Err(TypeError::Unsupported(format!(
                    "variadic callable type parameter '{}' in '{generic_name}'",
                    name.trim_start_matches('*')
                )));
            };
            let contract = substitute(bound, &subst);
            let Some(actual_contract) = self.declared_callable_contract(actual) else {
                return Err(TypeError::TraitNotSatisfied {
                    param: name.clone(),
                    ty: actual.to_string(),
                    trait_name: contract.to_string(),
                    reason: Some(
                        "requires one monomorphic callable type or nominal callable conformance"
                            .to_string(),
                    ),
                });
            };
            if !callable_bound_accepts(&actual_contract, &contract) {
                return Err(TypeError::TraitNotSatisfied {
                    param: name.clone(),
                    ty: actual.to_string(),
                    trait_name: contract.to_string(),
                    reason: Some(format!(
                        "declared callable contract '{}' is incompatible",
                        actual_contract
                    )),
                });
            }
        }
        Ok(())
    }

    /// The selected `@implicit` constructor for a conversion, when one
    /// applies. The second component is `Some(loan mutability)` when that
    /// constructor borrows its argument through a `ref [origin]` parameter (a
    /// view construction): the caller records the source borrow so the
    /// conversion result's origin refines to the source expression's place
    /// (temporary-origin inference).
    fn implicit_conversion_target(
        &self,
        from: &Ty,
        to: &Ty,
    ) -> Result<Option<(String, Option<bool>)>, TypeError> {
        Ok(self
            .implicit_conversion_constructor(from, to)?
            .map(|(target, source_borrow, _)| (target, source_borrow)))
    }

    /// Whether storing a `found` value where `expected` is declared goes
    /// through an implicit converting constructor that borrows its source
    /// (a view construction such as `var s: Span[Int, _] = xs`). The value
    /// consumed at that storage boundary is the conversion temporary, so the
    /// source place is not implicitly copied.
    pub(super) fn storage_conversion_borrows_source(
        &self,
        found: &Ty,
        expected: Option<&Ty>,
    ) -> bool {
        let Some(expected) = expected else {
            return false;
        };
        if self.value_coerces(found, expected) {
            return false;
        }
        matches!(
            self.implicit_conversion_constructor(found, expected),
            Ok(Some((_, _, false)))
        )
    }

    /// The selected implicit converting constructor from `from` to `to`, its
    /// `ref`-parameter source-borrow mutability, and whether it consumes its
    /// source (a `var`/`deinit` parameter).
    fn implicit_conversion_constructor(
        &self,
        from: &Ty,
        to: &Ty,
    ) -> Result<Option<(String, Option<bool>, bool)>, TypeError> {
        let Ty::Struct(name, args) = to else {
            return Ok(None);
        };
        let Some(info) = self.structs.get(name) else {
            return Ok(None);
        };
        if info.decls.len() != args.len() {
            return Ok(None);
        }
        let subst = struct_subst(&info.decls, args);
        let Some(constructors) = info.methods.get("__init__") else {
            return Ok(None);
        };
        let matches = constructors
            .iter()
            .filter(|sig| {
                sig.implicit
                    && sig.params.len() == 1
                    && coerces(from, &substitute(&sig.params[0], &subst))
            })
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => Ok(None),
            [sig] => {
                let target = if constructors.len() == 1 {
                    name.clone()
                } else {
                    method_lowered_name(name, "__init__", sig, self.self_instance_ty(name).as_ref())
                };
                let source_borrow = sig
                    .ref_params
                    .first()
                    .and_then(|signature| signature.as_ref())
                    .map(|signature| signature.mutability == crate::origin::SigMutability::Mutable);
                let consumes_source = matches!(
                    sig.conventions.first().copied().flatten(),
                    Some(crate::ast::ArgConvention::Var | crate::ast::ArgConvention::Deinit)
                );
                Ok(Some((target, source_borrow, consumes_source)))
            }
            _ => Err(TypeError::BadCall {
                func: name.clone(),
                reason: format!("ambiguous implicit conversion from '{from}' to '{to}'"),
            }),
        }
    }

    /// Record only a constructor-based implicit conversion (the `@implicit`
    /// single-parameter constructor), never a plain value coercion. The
    /// construction/storage checks use this so a failure of the stricter
    /// storage rule cannot be masked by a permissive value coercion (for
    /// example a capturing closure erasing into a plain `def` field).
    fn record_constructor_conversion(
        &self,
        expression: &Expr,
        from: &Ty,
        to: &Ty,
    ) -> Result<bool, TypeError> {
        let Some((target, source_borrow)) = self.implicit_conversion_target(from, to)? else {
            return Ok(false);
        };
        self.record_selected_conversion(expression, target, source_borrow);
        Ok(true)
    }

    /// Install one selected converting constructor plus, for a `ref`-parameter
    /// (view) constructor, the source-borrow fact that refines the conversion
    /// temporary's origin to the source expression's place.
    fn record_selected_conversion(
        &self,
        expression: &Expr,
        target: String,
        source_borrow: Option<bool>,
    ) {
        let span = expression.source_span();
        if let Some(mutable) = source_borrow {
            self.conversion_source_borrows
                .borrow_mut()
                .insert(span.clone(), mutable);
        }
        self.implicit_conversions.borrow_mut().insert(span, target);
    }

    fn record_implicit_conversion(
        &self,
        expression: &Expr,
        from: &Ty,
        to: &Ty,
    ) -> Result<bool, TypeError> {
        if let Ty::Overload(candidates) = from {
            let matches: Vec<_> = candidates
                .iter()
                .filter(|candidate| self.value_coerces(candidate, to))
                .filter_map(|candidate| {
                    let ExprKind::Identifier(name) = &expression.kind else {
                        return None;
                    };
                    callable_lowered_name(name, candidate).map(|target| (candidate, target))
                })
                .collect();
            return match matches.as_slice() {
                [(_, target)] => {
                    self.overload_targets
                        .borrow_mut()
                        .insert(expression.source_span(), target.clone());
                    Ok(true)
                }
                [] => Ok(false),
                _ => Err(TypeError::BadCall {
                    func: "overloaded callable value".to_string(),
                    reason: format!("multiple overloads fit expected type '{to}'"),
                }),
            };
        }
        if let Ty::Simd { dtype, width: 1 } = to
            && splats_to(from, *dtype)
        {
            self.record_literal_materializations(expression, from, to)?;
            return Ok(true);
        }
        if self.value_coerces(from, to) {
            self.record_literal_materializations(expression, from, to)?;
            return Ok(true);
        }
        let Some((target, source_borrow)) = self.implicit_conversion_target(from, to)? else {
            return Ok(false);
        };
        self.record_selected_conversion(expression, target, source_borrow);
        Ok(true)
    }

    /// Retain every exact-literal boundary selected by contextual typing.  The
    /// recursion matters for aggregates: `(1, 2.0)` materialized as
    /// `Tuple[Int, Float64]` has two scalar boundaries, not one tuple cast.
    fn record_literal_materializations(
        &self,
        expression: &Expr,
        from: &Ty,
        to: &Ty,
    ) -> Result<(), TypeError> {
        let scalar_boundary = matches!(from, Ty::IntLiteral | Ty::FloatLiteral)
            && matches!(
                to,
                Ty::Int | Ty::UInt | Ty::Float64 | Ty::Simd { width: 1, .. }
            );
        if scalar_boundary {
            if let Some(value) = self.exact_literal_value(expression)
                && !self.literal_value_fits_target(&value, to)
            {
                return Err(TypeError::TypeMismatch {
                    expected: to.to_string(),
                    found: from.to_string(),
                    context: format!("numeric literal materialization of '{value}'"),
                });
            }
            self.operation_adjustments.borrow_mut().insert(
                expression.source_span(),
                crate::checked::SemanticAdjustment::MaterializeLiteral(to.clone()),
            );
            return Ok(());
        }

        // A public Tuple literal is discovered under the canonical nominal
        // name and checked again under its generated specialization name. The
        // names are implementation identities; literal materialization follows
        // the retained element types across that handoff.
        if let (Some(actual), Some(expected)) = (tuple_elements(from), tuple_elements(to))
            && actual.len() == expected.len()
        {
            let values = match &expression.kind {
                ExprKind::TupleLit(values) => Some(values.as_slice()),
                ExprKind::Call { name, args, .. } if name == "Tuple" => Some(args.as_slice()),
                _ => None,
            };
            if let Some(values) = values {
                for ((value, actual), expected) in values.iter().zip(actual).zip(expected) {
                    self.record_literal_materializations(value, actual, expected)?;
                }
            }
            return Ok(());
        }

        match (from, to) {
            (Ty::Tuple(actual), Ty::Tuple(expected)) if actual.len() == expected.len() => {
                let values = match &expression.kind {
                    ExprKind::TupleLit(values) => Some(values.as_slice()),
                    ExprKind::Call { name, args, .. } if name == "Tuple" => Some(args.as_slice()),
                    _ => None,
                };
                if let Some(values) = values {
                    for ((value, actual), expected) in values.iter().zip(actual).zip(expected) {
                        self.record_literal_materializations(value, actual, expected)?;
                    }
                }
            }
            (Ty::ComptimeList(actual), Ty::ComptimeList(expected)) => {
                let values = match &expression.kind {
                    ExprKind::ListLit(values) => Some(values.as_slice()),
                    ExprKind::Call { name, args, .. } if name == "List" => Some(args.as_slice()),
                    _ => None,
                };
                if let Some(values) = values {
                    for value in values {
                        self.record_literal_materializations(value, actual, expected)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn exact_literal_value(&self, expression: &Expr) -> Option<CtValue> {
        match self.eval_associated_ct(expression, &HashMap::new()).ok()? {
            value @ (CtValue::IntLiteral(_) | CtValue::FloatLiteral(_)) => Some(value),
            _ => None,
        }
    }

    fn literal_value_fits_target(&self, value: &CtValue, target: &Ty) -> bool {
        match (value, target) {
            (CtValue::IntLiteral(_), Ty::Simd { dtype, width: 1 }) => {
                int_literal_materializes_to_dtype(*dtype)
            }
            (CtValue::FloatLiteral(_), Ty::Simd { dtype, width: 1 }) => dtype.is_float(),
            (value, Ty::Int | Ty::UInt | Ty::Float64) => {
                value.clone().materialize_as(target).is_some()
            }
            _ => false,
        }
    }

    /// An ordinary value use of a reference-returning expression owns an
    /// independent value: MIR lowers it to `ReadRef` followed by `CopyValue`.
    /// Reference-bearing contexts are recorded in `reference_value_uses` and
    /// retain the handle instead. Validate the remaining reads after the whole
    /// tree has been checked so a later enclosing context can mark a nested
    /// result as a retained handle before this distinction is enforced.
    fn check_reference_result_reads(&self) -> Result<(), TypeError> {
        let operations = self.operation_adjustments.borrow();
        let retained_handles = self.reference_value_uses.borrow();
        let copyable_reads = self.copyable_reference_result_reads.borrow();
        let borrowed_call_reads = self.borrowed_read_call_places.borrow();
        let borrowed_receivers = self.borrowed_reference_receivers.borrow();
        for (span, adjustment) in operations.iter() {
            let crate::checked::SemanticAdjustment::ReferenceResult { reference } = adjustment
            else {
                continue;
            };
            if retained_handles.contains_key(span)
                || copyable_reads.contains(span)
                || borrowed_call_reads.contains(span)
                || borrowed_receivers.contains(span)
                || self.is_implicitly_copyable(&reference.referent)
            {
                continue;
            }
            let context = "ordinary value read through a reference result".to_string();
            if !self.is_copyable(&reference.referent) {
                return Err(TypeError::NonCopyable {
                    ty: reference.referent.to_string(),
                    context,
                });
            }
            return Err(TypeError::ImplicitCopy {
                ty: reference.referent.to_string(),
                context,
                transferable: false,
                copyable: true,
            });
        }
        Ok(())
    }

    /// Compiler-generated keyword collectors use the bundled self-hosted
    /// `StringDict`. Its current `List`/`DictEntry` implementation is copy-based,
    /// so accept only element types that it can store without duplicating a
    /// linear value. Reference-bearing values also wait for collector origin
    /// metadata instead of silently losing their loans at the call boundary.
    fn kwargs_collector_ty(&self, element: Ty, context: &str) -> Result<Ty, TypeError> {
        if !self.is_copyable(&element) {
            return Err(TypeError::TraitNotSatisfied {
                param: "V".to_string(),
                ty: element.to_string(),
                trait_name: "Copyable".to_string(),
                reason: Some(format!(
                    "{context} is materialized as StringDict[V], whose current storage is copy-based"
                )),
            });
        }
        if self.type_contains_reference(&element) {
            return Err(TypeError::Unsupported(format!(
                "{context} cannot contain references until keyword collectors carry origin metadata"
            )));
        }
        Ok(Ty::Struct(
            "StringDict".to_string(),
            vec![TyArg::Ty(element)],
        ))
    }

    /// Require `expr` to have type `Bool` (used for `if`/`while` conditions).
    fn expect_bool(&self, expr: &Expr, context: &str) -> Result<(), TypeError> {
        let ty = self.infer(expr)?;
        if ty == Ty::Bool
            || matches!(
                ty,
                Ty::Simd {
                    dtype: crate::ast::Dtype::Bool,
                    width: 1
                }
            )
        {
            Ok(())
        } else {
            Err(TypeError::TypeMismatch {
                expected: "Bool".to_string(),
                found: ty.to_string(),
                context: context.to_string(),
            })
        }
    }
}

impl Default for Checker {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, PartialEq)]
struct MethodSig {
    decls: Vec<ParamDecl>,
    availability: Vec<GenericConstraint>,
    has_self: bool,
    /// Regular parameters only; variadic element type is stored separately.
    params: Vec<Ty>,
    names: Vec<String>,
    required: Vec<bool>,
    variadic: Option<Box<Ty>>,
    variadic_index: Option<usize>,
    kw_variadic: Option<Box<Ty>>,
    kw_variadic_index: Option<usize>,
    positional_only: Option<usize>,
    keyword_only: Option<usize>,
    conventions: Vec<Option<ArgConvention>>,
    ret: Ty,
    raises: bool,
    error: Option<Box<Ty>>,
    /// Receiver convention. `None` means plain read-only `self`; explicit
    /// conventions (`mut`, `var`, `ref`, ...) are preserved so trait
    /// requirements can compare them exactly. Today only `mut self` changes call
    /// checking behavior.
    self_convention: Option<crate::ast::ArgConvention>,
    ref_params: Vec<Option<crate::origin::RefSig>>,
    ref_return: Option<crate::origin::RefSig>,
    implicit: bool,
    /// Origin parameters the body writes through via a parametric-mut ref
    /// field subscript (`self.field[i] = v`). The write is legal only for
    /// instantiations binding the origin to a mutable source, judged at each
    /// call site against the receiver's concrete origin arguments.
    parametric_origin_writes: Vec<crate::origin::OriginParamId>,
}

impl MethodSig {
    fn intrinsic(params: Vec<Ty>, ret: Ty) -> MethodSig {
        let len = params.len();
        MethodSig {
            decls: Vec::new(),
            availability: Vec::new(),
            has_self: true,
            params,
            names: (0..len).map(|i| format!("arg{i}")).collect(),
            required: vec![true; len],
            variadic: None,
            variadic_index: None,
            kw_variadic: None,
            kw_variadic_index: None,
            positional_only: None,
            keyword_only: None,
            conventions: vec![None; len],
            ret,
            raises: false,
            error: None,
            self_convention: None,
            ref_params: vec![None; len],
            ref_return: None,
            implicit: false,
            parametric_origin_writes: Vec::new(),
        }
    }
}

/// Whether control can leave the owned iterator before exhaustion. A `break`
/// belongs to the nearest loop, while return/raise escape through every nested
/// loop. This is used only when residual elements cannot be deleted implicitly.
fn block_can_escape_owned_iteration(statements: &[Stmt], nested_loops: usize) -> bool {
    statements.iter().any(|statement| match &statement.kind {
        StmtKind::Break => nested_loops == 0,
        StmtKind::Return(_) | StmtKind::Raise(_) => true,
        StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
            branches
                .iter()
                .any(|(_, body)| block_can_escape_owned_iteration(body, nested_loops))
                || orelse
                    .as_ref()
                    .is_some_and(|body| block_can_escape_owned_iteration(body, nested_loops))
        }
        StmtKind::While { body, orelse, .. } | StmtKind::For { body, orelse, .. } => {
            block_can_escape_owned_iteration(body, nested_loops + 1)
                || orelse
                    .as_ref()
                    .is_some_and(|body| block_can_escape_owned_iteration(body, nested_loops))
        }
        StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } => {
            block_can_escape_owned_iteration(body, nested_loops)
                || except
                    .as_ref()
                    .is_some_and(|(_, body)| block_can_escape_owned_iteration(body, nested_loops))
                || orelse
                    .as_ref()
                    .is_some_and(|body| block_can_escape_owned_iteration(body, nested_loops))
                || finalbody
                    .as_ref()
                    .is_some_and(|body| block_can_escape_owned_iteration(body, nested_loops))
        }
        StmtKind::With { body, .. } => block_can_escape_owned_iteration(body, nested_loops),
        // Nested declarations do not execute as part of the loop body.
        StmtKind::Def { .. } | StmtKind::Struct { .. } | StmtKind::Trait { .. } => false,
        _ => false,
    })
}

/// The required kind/type of a trait `comptime NAME: Annotation` member.
#[derive(Clone, PartialEq)]
enum CtMemberReq {
    /// A compile-time value whose value type must match this type.
    Value(Box<Ty>),
    /// A compile-time type value whose type must conform to these trait bounds.
    /// `params` is non-empty for a parameterized associated type such as
    /// `comptime IteratorType[iterable_mut: Bool, //, iterable_origin:
    /// Origin[mut=iterable_mut]]: Iterator`; the raw `TypeParam`s are retained
    /// (rather than classified `ParamDecl`s) because `classify_params` erases
    /// origin parameters, which the application arity check needs.
    Type {
        bounds: Vec<String>,
        params: Vec<crate::ast::TypeParam>,
    },
}

enum OverloadSelect {
    NoMatch,
    Ambiguous,
}

fn ct_integer(value: &CtValue) -> Option<crate::literal::IntLiteral> {
    match value {
        CtValue::Int(value) => Some((*value).into()),
        CtValue::UInt(value) => Some((*value).into()),
        CtValue::IntLiteral(value) => Some(value.clone()),
        _ => None,
    }
}

/// A deliberately small implication relation for declaration availability.
/// It proves only facts which are syntactically present in a positive
/// conjunction (plus exact predicates).  In particular it does not turn a
/// failed symbolic evaluation, a negation, or either arm of a disjunction into
/// an assumption.
fn generic_constraint_implies(
    premise: &GenericConstraint,
    consequence: &GenericConstraint,
) -> bool {
    if let GenericConstraint::WithMessage(condition, _) = premise {
        return generic_constraint_implies(condition, consequence);
    }
    if let GenericConstraint::WithMessage(condition, _) = consequence {
        return generic_constraint_implies(premise, condition);
    }
    if premise == consequence || matches!(consequence, GenericConstraint::Bool(true)) {
        return true;
    }
    match (premise, consequence) {
        (_, GenericConstraint::And(left, right)) => {
            generic_constraint_implies(premise, left) && generic_constraint_implies(premise, right)
        }
        (GenericConstraint::And(left, right), _) => {
            generic_constraint_implies(left, consequence)
                || generic_constraint_implies(right, consequence)
        }
        _ => false,
    }
}

/// Fold a clause list into one right-nested conjunction for truth-only
/// operations such as implication. Storage keeps the un-folded list so each
/// clause retains its own diagnostic message.
fn fold_constraint_conjunction(constraints: &[GenericConstraint]) -> GenericConstraint {
    let mut folded: Option<GenericConstraint> = None;
    for constraint in constraints.iter().rev() {
        folded = Some(match folded {
            None => constraint.clone(),
            Some(rest) => GenericConstraint::And(Box::new(constraint.clone()), Box::new(rest)),
        });
    }
    folded.unwrap_or(GenericConstraint::Bool(true))
}

/// The checked signature of a struct, kept in the checker's registry.
struct StructInfo {
    /// Compile-time parameters (type and value); empty for a non-generic struct.
    decls: Vec<ParamDecl>,
    /// Raw source parameter list as declared, including Origin/OriginSet
    /// parameters and their `mut=` Bool binders, which `decls` erases.
    /// Direct applications consult it to accept, validate, and erase explicit
    /// origin arguments (`EntryIter[K, V, some_origin]`).
    source_params: Vec<crate::ast::TypeParam>,
    /// Concrete semantic arguments retained by an erased compiler-generated
    /// specialization. Public `Tuple[*Ts]` is emitted as a parameter-free
    /// implementation struct, but its checked identity must still carry the
    /// element types selected for `Ts`.
    fixed_arguments: Option<Vec<TyArg>>,
    /// Traits this struct declares conformance to (verified at definition).
    conforms: Vec<String>,
    callable_conformance: Option<Ty>,
    /// Exact lowered `__call__` method selected by the declared callable
    /// conformance. This is distinct from the callable contract: overloaded
    /// methods with the same arity require the signature-qualified target to
    /// survive into indirect-call MIR.
    callable_target: Option<String>,
    conformance_conditions: HashMap<String, Expr>,
    /// Declared fields, in order (drives the fieldwise constructor).
    fields: Vec<(String, Ty)>,
    /// For each field whose SOURCE annotation applied origin-binder
    /// arguments (`var iter: EntryIter[Self.o2]`), the (callee origin-param
    /// index, enclosing origin-param index) pairs the application bound —
    /// both in their declaration lists' full-index (OriginParamId) domain.
    /// Origin arguments are erased from checked identity, so this is the
    /// surviving record delegated-call origin clauses resolve binder
    /// correspondences through.
    field_origin_arguments: HashMap<String, Vec<(u32, u32)>>,
    /// Associated compile-time facts declared by `comptime NAME = ...` in the
    /// struct body. These live on the type, not on runtime instances.
    associated: HashMap<String, CtValue>,
    /// Availability constraints for monomorphic associated members, one entry
    /// per trailing `where` clause. They are evaluated against the enclosing
    /// struct arguments at projection time.
    associated_constraints: HashMap<String, Vec<GenericConstraint>>,
    /// Parameterized associated types declared by `comptime NAME[params] = body`.
    /// Unlike `associated`, the body cannot be evaluated eagerly (it references
    /// the member's own parameters); it is lowered once to a symbolic template
    /// and substituted per application. See [`ParameterizedMember`].
    parameterized_associated: HashMap<String, ParameterizedMember>,
    methods: HashMap<String, Vec<MethodSig>>,
    fieldwise_init: bool,
    explicit_destroy_message: Option<String>,
    explicit_destructors: HashMap<String, bool>,
}

/// A generic top-level alias (`comptime Alias[params] = Type` or a Bool
/// proposition). The parameters are classified `ParamDecl`s — trailing `where`
/// clauses attach to the last one — so each type-bodied application validates
/// arity, bounds, defaults, and declaration constraints through the same
/// `resolve_use_params` contract as a struct application. A type body is
/// lowered once to a symbolic template (`Ty::Param` / `CtValue::Param`) and
/// substituted per application; a Bool body is lowered once to a symbolic
/// [`GenericConstraint`] and inlined into the consuming proposition per
/// application. Aliases lower sequentially at declaration, so a body may
/// reference only already-declared names: self-reference fails as an unknown
/// type, and an alias expanding an earlier alias bakes the expansion into its
/// template. Origin parameters are rejected at declaration, so no origin
/// bindings exist.
#[derive(Clone)]
struct ComptimeAlias {
    decls: Vec<ParamDecl>,
    body: AliasBody,
}

/// The lowered body of a [`ComptimeAlias`]: a symbolic type template, or a
/// symbolic Bool proposition (a predicate alias, usable exactly where
/// `conforms_to`/`IsTrivially*` propositions are — never in type positions).
#[derive(Clone)]
enum AliasBody {
    Type(Box<Ty>),
    Predicate(Box<GenericConstraint>),
}

/// A parameterized associated type a conforming struct defines
/// (`comptime Buf[n: Int] = Fixed[n]`). The body is lowered once with the
/// member's own parameters in scope, so the resulting `template` carries them
/// symbolically (`Ty::Param`, `CtValue::Param`, `Origin::Param`); concrete
/// resolution substitutes an application's arguments into it. The raw
/// `TypeParam`s are retained (rather than classified `ParamDecl`s) so the
/// argument-to-parameter binding can distinguish type, value, and origin kinds.
#[derive(Clone)]
struct ParameterizedMember {
    params: Vec<crate::ast::TypeParam>,
    template: Ty,
    /// Constraints (one per trailing `where` clause) evaluated against both
    /// enclosing struct arguments and the member application's explicit
    /// arguments.
    availability: Vec<GenericConstraint>,
    /// The index the member's parameters started at in `enclosing_type_params`
    /// while the template was lowered. An origin parameter at member position `k`
    /// therefore appears in the template as `Origin::Param(param_base + k)`, which
    /// concrete substitution uses to bind the application's origin argument.
    param_base: usize,
}

/// A struct's checked associated compile-time members: the eagerly evaluated
/// monomorphic values, and the parameterized members lowered to their symbolic
/// templates for later concrete substitution.
type StructAssociatedMembers = (
    HashMap<String, CtValue>,
    HashMap<String, Vec<GenericConstraint>>,
    HashMap<String, ParameterizedMember>,
);

#[derive(Clone, Copy)]
struct DependentIndexAccessorFamily {
    place: &'static str,
    value: &'static str,
}

fn canonical_generic_signature(decls: &[ParamDecl], params: &[Ty]) -> (Vec<ParamDecl>, Vec<Ty>) {
    let identity_constraints = |constraints: &[GenericConstraint]| {
        constraints
            .iter()
            .map(|constraint| match constraint {
                GenericConstraint::WithMessage(condition, _) => (**condition).clone(),
                constraint => constraint.clone(),
            })
            .collect()
    };
    let mut subst = HashMap::new();
    let mut value_names = HashMap::new();
    let canonical_decls = decls
        .iter()
        .enumerate()
        .map(|(index, decl)| match decl {
            ParamDecl::Type {
                name,
                bounds,
                callable_bound,
                default: _,
                infer_only: _,
                variadic,
                constraints,
            } => {
                let canonical_name = format!("${index}");
                let canonical_callable_bound = callable_bound.as_ref().map(|bound| {
                    Box::new(rename_dependent_parameters(
                        &substitute(bound, &subst),
                        &value_names,
                    ))
                });
                subst.insert(
                    name.clone(),
                    Ty::Param {
                        name: canonical_name.clone(),
                        bounds: bounds.clone(),
                        callable_bound: canonical_callable_bound.clone(),
                    },
                );
                ParamDecl::Type {
                    name: canonical_name,
                    bounds: bounds.clone(),
                    callable_bound: canonical_callable_bound,
                    // Binder defaults and the `//` inference marker govern a
                    // call through the contract; current Mojo does not make
                    // either part of generic callable conformance identity.
                    default: None,
                    infer_only: false,
                    variadic: *variadic,
                    constraints: identity_constraints(constraints),
                }
            }
            ParamDecl::Value {
                name,
                ty,
                default: _,
                callable_default: _,
                infer_only: _,
                variadic,
                constraints,
                ..
            } => {
                let canonical_name = format!("${index}");
                let canonical_ty =
                    rename_dependent_parameters(&substitute(ty, &subst), &value_names);
                value_names.insert(
                    name.trim_start_matches('*').to_string(),
                    canonical_name.clone(),
                );
                ParamDecl::Value {
                    name: canonical_name,
                    ty: Box::new(canonical_ty),
                    default: None,
                    callable_default: None,
                    infer_only: false,
                    variadic: *variadic,
                    constraints: identity_constraints(constraints),
                }
            }
        })
        .collect();
    let canonical_params = params
        .iter()
        .map(|ty| rename_dependent_parameters(&substitute(ty, &subst), &value_names))
        .collect();
    // Second pass: alpha-rename binder references INSIDE the retained
    // constraints, so `def[w: Int](…) where w > 0` and `def[n: Int](…) where
    // n > 0` share one canonical identity. The maps are complete only after
    // the fold above (a clause on the last binder may reference any of them);
    // names the contract does not bind (an enclosing declaration's
    // parameters) stay as-is and correctly distinguish contracts.
    let mut binder_names: HashMap<String, String> = value_names.clone();
    for (name, ty) in &subst {
        if let Ty::Param {
            name: canonical, ..
        } = ty
        {
            binder_names.insert(name.clone(), canonical.clone());
        }
    }
    let mut canonical_decls: Vec<ParamDecl> = canonical_decls;
    for decl in &mut canonical_decls {
        match decl {
            ParamDecl::Type { constraints, .. } | ParamDecl::Value { constraints, .. } => {
                *constraints = constraints
                    .iter()
                    .map(|constraint| {
                        rename_constraint_parameters(
                            constraint,
                            &binder_names,
                            &subst,
                            &value_names,
                        )
                    })
                    .collect();
            }
        }
    }
    (canonical_decls, canonical_params)
}

/// Alpha-rename the binder references inside one canonicalized constraint:
/// `param`-shaped fields rename through `binder_names` (falling back to the
/// pack-trimmed spelling), and embedded types canonicalize exactly like
/// signature types.
fn rename_constraint_parameters(
    constraint: &GenericConstraint,
    binder_names: &HashMap<String, String>,
    subst: &HashMap<String, Ty>,
    value_names: &HashMap<String, String>,
) -> GenericConstraint {
    let rename = |name: &str| -> String {
        if let Some(canonical) = binder_names.get(name) {
            return canonical.clone();
        }
        let trimmed = name.trim_start_matches('*');
        if let Some(canonical) = binder_names.get(trimmed) {
            return canonical.clone();
        }
        name.to_string()
    };
    let operand = |operand: &crate::types::ConstraintOperand| -> crate::types::ConstraintOperand {
        use crate::types::ConstraintOperand;
        match operand {
            ConstraintOperand::Param(name) => ConstraintOperand::Param(rename(name)),
            ConstraintOperand::PackLength(name) => ConstraintOperand::PackLength(rename(name)),
            ConstraintOperand::Value(value) => ConstraintOperand::Value(value.clone()),
            ConstraintOperand::Type(ty) => ConstraintOperand::Type(rename_dependent_parameters(
                &substitute(ty, subst),
                value_names,
            )),
        }
    };
    let recurse = |inner: &GenericConstraint| {
        rename_constraint_parameters(inner, binder_names, subst, value_names)
    };
    match constraint {
        GenericConstraint::WithMessage(inner, message) => {
            GenericConstraint::WithMessage(Box::new(recurse(inner)), message.clone())
        }
        GenericConstraint::Conforms { param, trait_name } => GenericConstraint::Conforms {
            param: rename(param),
            trait_name: trait_name.clone(),
        },
        GenericConstraint::ConformsPack { param, trait_name } => GenericConstraint::ConformsPack {
            param: rename(param),
            trait_name: trait_name.clone(),
        },
        GenericConstraint::PackPredicate {
            param,
            predicate,
            all,
        } => GenericConstraint::PackPredicate {
            param: rename(param),
            predicate: predicate.clone(),
            all: *all,
        },
        GenericConstraint::PackContains { param, element } => GenericConstraint::PackContains {
            param: rename(param),
            element: operand(element),
        },
        GenericConstraint::Trivial(kind, inner) => {
            GenericConstraint::Trivial(*kind, operand(inner))
        }
        GenericConstraint::Eq(a, b) => GenericConstraint::Eq(operand(a), operand(b)),
        GenericConstraint::Ne(a, b) => GenericConstraint::Ne(operand(a), operand(b)),
        GenericConstraint::Lt(a, b) => GenericConstraint::Lt(operand(a), operand(b)),
        GenericConstraint::Le(a, b) => GenericConstraint::Le(operand(a), operand(b)),
        GenericConstraint::Gt(a, b) => GenericConstraint::Gt(operand(a), operand(b)),
        GenericConstraint::Ge(a, b) => GenericConstraint::Ge(operand(a), operand(b)),
        GenericConstraint::And(a, b) => {
            GenericConstraint::And(Box::new(recurse(a)), Box::new(recurse(b)))
        }
        GenericConstraint::Or(a, b) => {
            GenericConstraint::Or(Box::new(recurse(a)), Box::new(recurse(b)))
        }
        GenericConstraint::Not(inner) => GenericConstraint::Not(Box::new(recurse(inner))),
        GenericConstraint::Bool(value) => GenericConstraint::Bool(*value),
    }
}

fn canonical_generic_parameter_shape(
    decls: &[ParamDecl],
    params: &[Ty],
    variadic: Option<&Ty>,
    kw_variadic: Option<&Ty>,
) -> (Vec<ParamDecl>, Vec<Ty>, Option<Ty>, Option<Ty>) {
    let mut signature = params.to_vec();
    let variadic_index = variadic.map(|parameter| {
        let index = signature.len();
        signature.push(parameter.clone());
        index
    });
    let kw_variadic_index = kw_variadic.map(|parameter| {
        let index = signature.len();
        signature.push(parameter.clone());
        index
    });
    let (decls, signature) = canonical_generic_signature(decls, &signature);
    (
        decls,
        signature[..params.len()].to_vec(),
        variadic_index.map(|index| signature[index].clone()),
        kw_variadic_index.map(|index| signature[index].clone()),
    )
}

/// The checked signature of a trait: required methods plus associated
/// compile-time facts. A method requirement's signature may mention
/// `Ty::SelfType` (the conforming type).
struct TraitInfo {
    refines: Vec<String>,
    methods: HashMap<String, Vec<MethodSig>>,
    comptime_members: HashMap<String, CtMemberReq>,
    /// Per-member declaration constraints, one entry per trailing `where`
    /// clause on the requirement.
    comptime_constraints: HashMap<String, Vec<GenericConstraint>>,
}

fn callable_parameter_count(ty: &Ty) -> Option<usize> {
    match ty {
        Ty::Func { params, .. } => Some(params.len()),
        Ty::GenericFunc { params, .. } => Some(params.len()),
        _ => None,
    }
}

fn method_arity_range(sig: &MethodSig) -> (usize, usize) {
    (sig.params.len(), sig.params.len())
}

fn guaranteed_conformance_atoms(
    constraint: &GenericConstraint,
    output: &mut Vec<(String, String)>,
) {
    match constraint {
        GenericConstraint::WithMessage(condition, _) => {
            guaranteed_conformance_atoms(condition, output);
        }
        GenericConstraint::Conforms { param, trait_name } => {
            let atom = (param.clone(), trait_name.clone());
            if !output.contains(&atom) {
                output.push(atom);
            }
        }
        // `where IsTrivially*[T]` guarantees the facet (and, through the
        // assumption table's implications, the base capability) inside the
        // body, recorded under the predicate's spelling.
        GenericConstraint::Trivial(kind, crate::types::ConstraintOperand::Param(param)) => {
            let atom = (
                param.clone(),
                crate::types::trivial_predicate_spelling(*kind).to_string(),
            );
            if !output.contains(&atom) {
                output.push(atom);
            }
        }
        GenericConstraint::And(left, right) => {
            guaranteed_conformance_atoms(left, output);
            guaranteed_conformance_atoms(right, output);
        }
        // A disjunction, negation, comparison, or symbolic pack predicate does
        // not unconditionally refine one ordinary type parameter.
        _ => {}
    }
}

/// Overload identity works at the mangled-symbol level: the compile-time
/// `StringLiteral` and the nominal stdlib `String` deliberately share the
/// stable `String` symbol spelling, so two overloads differing only in that
/// pair collide and are rejected as redeclarations.
fn symbol_identity_ty(ty: &Ty) -> std::borrow::Cow<'_, Ty> {
    match ty {
        Ty::Struct(name, args)
            if args.is_empty() && crate::symbol::is_stdlib_string_struct(name) =>
        {
            std::borrow::Cow::Owned(Ty::StringLiteral)
        }
        other => std::borrow::Cow::Borrowed(other),
    }
}

fn symbol_equivalent_params(a: &[Ty], b: &[Ty]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(a, b)| symbol_identity_ty(a) == symbol_identity_ty(b))
}

fn same_callable_signature(a: &Ty, b: &Ty) -> bool {
    match (a, b) {
        (
            Ty::Func {
                params: ap,
                variadic: av,
                kw_variadic: akw,
                ..
            },
            Ty::Func {
                params: bp,
                variadic: bv,
                kw_variadic: bkw,
                ..
            },
        ) => symbol_equivalent_params(ap, bp) && av == bv && akw == bkw,
        (
            Ty::GenericFunc {
                decls: ad,
                params: ap,
                variadic: av,
                kw_variadic: akw,
                ..
            },
            Ty::GenericFunc {
                decls: bd,
                params: bp,
                variadic: bv,
                kw_variadic: bkw,
                ..
            },
        ) => {
            canonical_generic_parameter_shape(ad, ap, av.as_deref(), akw.as_deref())
                == canonical_generic_parameter_shape(bd, bp, bv.as_deref(), bkw.as_deref())
        }
        _ => false,
    }
}

/// Alpha-normalize a generic anonymous callable into its declaration list and
/// a monomorphic callable shape whose parameter occurrences use canonical
/// `$N` names.  Generic callable compatibility can then reuse the ordinary
/// directional callable-contract rules without making source binder spelling
/// part of the type identity.
fn erase_generic_callable_binders(callable: &Ty) -> Option<(Vec<ParamDecl>, Ty)> {
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
        return None;
    };

    let mut signature = params.clone();
    let variadic_index = variadic.as_ref().map(|parameter| {
        let index = signature.len();
        signature.push((**parameter).clone());
        index
    });
    let kw_variadic_index = kw_variadic.as_ref().map(|parameter| {
        let index = signature.len();
        signature.push((**parameter).clone());
        index
    });
    let return_index = signature.len();
    signature.push((**ret).clone());
    let error_index = error.as_ref().map(|error| {
        let index = signature.len();
        signature.push((**error).clone());
        index
    });
    let (decls, signature) = canonical_generic_signature(decls, &signature);

    Some((
        decls,
        Ty::Func {
            environment: environment.clone(),
            params: signature[..params.len()].to_vec(),
            names: names.clone(),
            ret: Box::new(signature[return_index].clone()),
            required: required.clone(),
            variadic: variadic_index.map(|index| Box::new(signature[index].clone())),
            kw_variadic: kw_variadic_index.map(|index| Box::new(signature[index].clone())),
            positional_only: *positional_only,
            keyword_only: *keyword_only,
            raises: *raises,
            error: error_index.map(|index| Box::new(signature[index].clone())),
            conventions: conventions.clone(),
            ref_params: ref_params.clone(),
            ref_return: ref_return.clone(),
            transfers: transfers.clone(),
        },
    ))
}

/// A concrete method candidate after receiver-type substitution and argument
/// scoring. Named fields keep overload resolution readable as it evolves.
/// One body's transfer-effect accumulation frame: the callable's key and
/// its ordered parameter owners, so accepted outliving stores abstract to
/// signature-relative effects.
struct TransferFrame {
    callable: String,
    param_owners: Vec<crate::origin::OwnerId>,
    /// Whether each parameter's convention borrows caller storage
    /// (`mut`/`ref`) rather than owning a moved value.
    param_borrowed: Vec<bool>,
    self_owner: Option<crate::origin::OwnerId>,
    /// Compile-time callable value parameters (decl names) in scope in this
    /// body — call-through recording keys on them.
    value_callables: Vec<String>,
    effects: Vec<crate::checked::TransferEffect>,
    call_throughs: Vec<crate::checked::CallThroughEffect>,
}

/// Bundled collection mutators store an argument into `self` through
/// pointer intrinsics the body-level store rule cannot see; their transfer
/// effects are seeded here (the declared-metadata pattern used for interior
/// projections) instead of inferred from bodies.
fn seeded_transfer_effects() -> HashMap<String, Vec<crate::checked::TransferEffect>> {
    use crate::origin::SigOrigin;
    let effect = |src: usize| {
        vec![crate::checked::TransferEffect {
            dest: SigOrigin::Self_,
            src: SigOrigin::Param(src),
            // `var` element parameters own a moved value: only carried
            // loans transfer, never the parameter slot's own storage.
            src_is_place: false,
            mutable: true,
        }]
    };
    HashMap::from([
        ("List.append".to_string(), effect(0)),
        ("List.insert".to_string(), effect(1)),
        ("List.__setitem__".to_string(), effect(1)),
    ])
}

struct MethodCallResolution {
    conversion_score: usize,
    slots: Vec<ArgSlot>,
    positional_overflow: Vec<usize>,
    keyword_overflow: Vec<usize>,
    variadic_element: Option<Ty>,
    keyword_element: Option<Ty>,
    conventions: Vec<Option<ArgConvention>>,
    self_convention: Option<ArgConvention>,
    return_type: Ty,
    result_adapter: Option<crate::checked::CheckedResultAdapter>,
    raises: bool,
    error: Option<Box<Ty>>,
    mutates_receiver: bool,
    consumes_receiver: bool,
    lowered_name: Option<String>,
    ref_params: Vec<Option<crate::origin::RefSig>>,
    ref_return: Option<crate::origin::RefSig>,
    param_types: Vec<Ty>,
    param_decls: Vec<ParamDecl>,
    /// See [`MethodSig::parametric_origin_writes`].
    parametric_origin_writes: Vec<crate::origin::OriginParamId>,
}

type SubscriptDescriptorPlan = (Vec<Option<SliceKind>>, bool);

/// How strictly a storage annotation must bind explicit origin slots.
/// `Full`: bare origin-slotted generics and partial applications both reject
/// (struct fields, uninitialized locals). `AllowBare`: a bare generic may
/// infer wholly from the binding's initializer, but a partial application
/// still cannot omit an origin slot (initialized locals — both halves
/// probe-attested against the pinned upstream). `Off`: ordinary resolution.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StorageStrictness {
    Off,
    AllowBare,
    Full,
}

fn ct_values_equal(left: &CtValue, right: &CtValue) -> bool {
    match (ct_integer(left), ct_integer(right)) {
        (Some(left), Some(right)) => left == right,
        _ => left == right,
    }
}

fn place_root_name(expr: &Expr) -> Option<&str> {
    match &expr.kind {
        ExprKind::Identifier(name) => Some(name),
        ExprKind::Member { object, .. }
        | ExprKind::Index { object, .. }
        | ExprKind::Slice { object, .. }
        | ExprKind::MultiIndex { object, .. } => place_root_name(object),
        ExprKind::TypeApply { name, .. } => Some(name),
        _ => None,
    }
}

fn place_has_index(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Index { .. } | ExprKind::Slice { .. } | ExprKind::MultiIndex { .. } => true,
        ExprKind::Member { object, .. } => place_has_index(object),
        _ => false,
    }
}

/// The lowered symbol the checker records as the resolved callee of an
/// overloaded free-function call — formatted by the canonical symbol module so
/// it names exactly the `MirFunction` the MIR emits for that definition.
fn callable_lowered_name(name: &str, ty: &Ty) -> Option<String> {
    let (params, variadic, kw_variadic) = match ty {
        Ty::Func {
            params,
            variadic,
            kw_variadic,
            ..
        }
        | Ty::GenericFunc {
            params,
            variadic,
            kw_variadic,
            ..
        } => (params, variadic, kw_variadic),
        _ => return None,
    };
    let signature_types: Vec<_> = params
        .iter()
        .chain(variadic.iter().map(Box::as_ref))
        .collect();
    let signature = crate::symbol::SignatureKey::from_tys(signature_types)
        .with_kw_variadic(kw_variadic.as_deref());
    Some(crate::symbol::function_symbol(name, &signature))
}

/// The lowered symbol of an overloaded method/constructor resolution, likewise
/// canonical. `sig.params` are the declared parameter types with `Self`
/// substituted to the enclosing struct at declaration time; `self_ty` (the
/// receiver struct's instance type) canonicalizes those occurrences back to
/// `Self`, so the key matches the MIR definition side, which mangles the bare
/// `Self` annotation. Pass `None` for abstract trait dispatch, whose `sig`
/// parameters retain `Ty::SelfType` and already spell `Self`.
fn method_lowered_name(
    type_name: &str,
    method: &str,
    sig: &MethodSig,
    self_ty: Option<&Ty>,
) -> String {
    // The variadic element participates in overload identity at its declared
    // position (the MIR symbol walks parameters in source order).
    let mut signature_types: Vec<&Ty> = sig.params.iter().collect();
    if let Some(element) = sig.variadic.as_deref() {
        let index = sig
            .variadic_index
            .unwrap_or(signature_types.len())
            .min(signature_types.len());
        signature_types.insert(index, element);
    }
    let signature_types = signature_types.into_iter();
    let keyword_names = match sig.keyword_only {
        Some(index) => sig.names[index..].to_vec(),
        None => Vec::new(),
    };
    let signature = crate::symbol::SignatureKey::from_tys_with_self(signature_types, self_ty)
        .with_kw_variadic(sig.kw_variadic.as_deref())
        .with_keyword_names(keyword_names);
    if crate::symbol::receiver_overloaded_method(method) {
        crate::symbol::receiver_method_symbol(type_name, method, sig.self_convention, &signature)
    } else {
        crate::symbol::method_symbol(type_name, method, &signature)
    }
}

fn callable_convention_accepts(
    actual: Option<ArgConvention>,
    contract: Option<ArgConvention>,
) -> bool {
    let actual = actual.unwrap_or(ArgConvention::Imm);
    let contract = contract.unwrap_or(ArgConvention::Imm);
    match (actual, contract) {
        // A read-only callee demands less access than a mutable callable
        // contract promises to supply, so it is a valid implementation.
        (ArgConvention::Imm, ArgConvention::Imm | ArgConvention::Mut) => true,
        (ArgConvention::Mut, ArgConvention::Mut) => true,
        // Ownership-changing and parametric-reference conventions retain their
        // exact ABI until their full subtyping rules are modeled.
        (actual, contract) => actual == contract,
    }
}

const CONVERSION_RANK: usize = 1 << 24;

const VARIADIC_RANK: usize = 1 << 16;

const SIGNATURE_LENGTH_RANK: usize = 1 << 8;

/// Source-level arguments attached to a method invocation. Keeping the runtime
/// and compile-time argument lists together prevents the two method-resolution
/// paths from slowly acquiring different call-shape parameters.
#[derive(Clone, Copy)]
struct MethodCallArguments<'a> {
    param_args: &'a [crate::ast::ParamArg],
    args: &'a [Expr],
    kwargs: &'a [crate::ast::KwArg],
    parameterized_syntax: bool,
    /// The caller separately records a more precise projected write, so a
    /// `mut self` call must not also invalidate every receiver interior.
    preserves_receiver_interiors: bool,
}

impl<'a> MethodCallArguments<'a> {
    fn ordinary(args: &'a [Expr], kwargs: &'a [crate::ast::KwArg]) -> Self {
        Self {
            param_args: &[],
            args,
            kwargs,
            parameterized_syntax: false,
            preserves_receiver_interiors: false,
        }
    }

    fn interior_preserving(args: &'a [Expr], kwargs: &'a [crate::ast::KwArg]) -> Self {
        Self {
            preserves_receiver_interiors: true,
            ..Self::ordinary(args, kwargs)
        }
    }

    fn parameterized(
        param_args: &'a [crate::ast::ParamArg],
        args: &'a [Expr],
        kwargs: &'a [crate::ast::KwArg],
    ) -> Self {
        Self {
            param_args,
            args,
            kwargs,
            parameterized_syntax: true,
            preserves_receiver_interiors: false,
        }
    }
}

type ReturnRefContract = (
    crate::origin::RefSig,
    Vec<crate::origin::OwnerId>,
    Option<crate::origin::OriginPlace>,
);

/// Materialize trait default methods into each conforming struct before semantic
/// checking. This keeps default dispatch static: downstream MIR sees an ordinary
/// struct method and needs no trait-object runtime machinery.
fn expand_trait_defaults(stmts: &[Stmt]) -> Result<Vec<Stmt>, TypeError> {
    #[derive(Clone)]
    struct TraitDefaults {
        refines: Vec<String>,
        methods: Vec<crate::ast::TraitMethod>,
    }

    fn defaults_for(
        name: &str,
        traits: &HashMap<String, TraitDefaults>,
        visiting: &mut HashSet<String>,
    ) -> Result<HashMap<String, Method>, TypeError> {
        if !visiting.insert(name.to_string()) {
            return Err(TypeError::Unsupported(format!(
                "cyclic trait refinement involving '{name}'"
            )));
        }
        let Some(info) = traits.get(name) else {
            visiting.remove(name);
            return Ok(HashMap::new());
        };
        let mut defaults = HashMap::new();
        for parent in &info.refines {
            for (method, implementation) in defaults_for(parent, traits, visiting)? {
                if defaults.insert(method.clone(), implementation).is_some() {
                    return Err(TypeError::Unsupported(format!(
                        "ambiguous inherited default method '{method}'"
                    )));
                }
            }
        }
        for method in &info.methods {
            let Some(body) = &method.default_body else {
                continue;
            };
            defaults.insert(
                method.name.clone(),
                Method {
                    name: method.name.clone(),
                    type_params: method.type_params.clone(),
                    has_self: true,
                    self_convention: method.self_convention,
                    self_origin: method.self_origin.clone(),
                    decorators: Vec::new(),
                    params: method.params.clone(),
                    positional_only: method.positional_only,
                    keyword_only: method.keyword_only,
                    raises: method.raises,
                    raises_type: method.raises_type.clone(),
                    ret: method.ret.clone(),
                    body: body.clone(),
                    where_clauses: method.where_clauses.clone(),
                },
            );
        }
        visiting.remove(name);
        Ok(defaults)
    }

    let traits: HashMap<_, _> = stmts
        .iter()
        .filter_map(|stmt| match &stmt.kind {
            StmtKind::Trait {
                name,
                refines,
                methods,
                ..
            } => Some((
                name.clone(),
                TraitDefaults {
                    refines: refines.clone(),
                    methods: methods.clone(),
                },
            )),
            _ => None,
        })
        .collect();
    let mut expanded = stmts.to_vec();
    for stmt in &mut expanded {
        let StmtKind::Struct {
            conforms, methods, ..
        } = &mut stmt.kind
        else {
            continue;
        };
        let explicit: HashSet<_> = methods.iter().map(|method| method.name.clone()).collect();
        let mut inherited = HashMap::<String, Method>::new();
        for trait_name in conforms.iter() {
            for (name, implementation) in defaults_for(trait_name, &traits, &mut HashSet::new())? {
                if explicit.contains(&name) {
                    continue;
                }
                if inherited.insert(name.clone(), implementation).is_some() {
                    return Err(TypeError::Unsupported(format!(
                        "ambiguous default method '{name}'; provide an explicit override"
                    )));
                }
            }
        }
        methods.extend(inherited.into_values());
    }
    Ok(expanded)
}

#[derive(Clone)]
struct CapturePolicy {
    /// Scope index at which the nested function's own locals begin.
    base: usize,
    function_name: String,
    declaration: SourceSpan,
    entries: HashMap<String, crate::ast::CaptureKind>,
    default: Option<crate::ast::CaptureKind>,
    /// Whether this policy belongs to a lambda expression's hidden definition
    /// (selects the lambda wording for capture-convention diagnostics).
    lambda: bool,
}

/// How one source-level `Origin` parameter is represented by the callable's
/// slot-relative reference contracts. Origin parameters are erased from
/// `Ty::Func`'s ordinary compile-time parameter list, so this checker-owned fact
/// is what lets a value expression such as `borrow[origin_of(value)]` bind that
/// parameter without reconstructing it from source later in MIR lowering.
#[derive(Clone)]
struct CallableOriginParam {
    name: String,
    slots: Vec<usize>,
}

/// One source compile-time parameter in a callable declaration. `Origin`
/// parameters are erased from `Ty::GenericFunc::decls`, so retaining this
/// ordered layout is necessary to split a mixed specialization such as
/// `borrow[Int, origin_of(value)]` without shifting the ordinary type argument.
#[derive(Clone)]
struct CallableSourceParam {
    name: String,
    infer_only: bool,
    origin: Option<usize>,
    ordinary: bool,
}

/// Origin-specialization metadata for one declaration in an overload set.
/// Entries are registered in the same order as `Ty::Overload` candidates.
#[derive(Clone)]
struct CallableOriginSignature {
    origins: Vec<CallableOriginParam>,
    source: Vec<CallableSourceParam>,
    /// Declaration constraints whose only binders were erased origin
    /// metadata, one per trailing `where` clause. They are checked after
    /// call-origin solving recovers the inferred Bool mutability arguments.
    availability: Vec<GenericConstraint>,
}

/// The raw-slot operations on `UnsafePointer` are an implementation privilege,
/// not source-language API. Linked expressions retain their exact source path;
/// only files physically shipped in the compiler's collection library receive
/// the checked adjustment that can lower these operations.
fn is_bundled_collection_source(source: Option<&str>) -> bool {
    let (Some(root), Some(source)) = (crate::module::bundled_root(), source) else {
        return false;
    };
    let stdlib = root.join("stdlib");
    let source = Path::new(source);
    source == stdlib.join("std/collections/list.mojo")
        || source == stdlib.join("list.mojo")
        || source == stdlib.join("std/collections/dict.mojo")
        || source == stdlib.join("dict.mojo")
        || source == stdlib.join("std/collections/array.mojo")
        || source == stdlib.join("std/optional.mojo")
        || source == stdlib.join("optional.mojo")
        || source == stdlib.join("std/memory.mojo")
}

/// Static `UnsafePointer[T].alloc[_aligned]` is the compiler's heap primitive,
/// retired from source-language API by the current layout-based model (the
/// audited head rejects it). `std/memory.mojo` is the single bundled crossing;
/// every other module — stdlib included — allocates through `std.memory`.
fn is_bundled_stdlib_source(source: Option<&str>) -> bool {
    let (Some(root), Some(source)) = (crate::module::bundled_root(), source) else {
        return false;
    };
    Path::new(source) == root.join("stdlib").join("std/memory.mojo")
}

/// Select the current Mojo parameter-index hook first, while retaining the
/// earlier spelling as an intentional source-compatibility fallback.
fn dependent_index_accessor_family(info: &StructInfo) -> Option<DependentIndexAccessorFamily> {
    if info.methods.contains_key("__getitem_param__$0") {
        Some(DependentIndexAccessorFamily {
            place: "__getitem_param__",
            value: "__getitem_param_value__",
        })
    } else if info.methods.contains_key("__getitem__$0") {
        Some(DependentIndexAccessorFamily {
            place: "__getitem__",
            value: "__getitem_value__",
        })
    } else {
        None
    }
}

/// The source-level pieces of a struct declaration passed through checking.
struct StructDeclaration<'a> {
    module: &'a Option<String>,
    name: &'a str,
    type_params: &'a [crate::ast::TypeParam],
    conforms: &'a [String],
    callable_conformance: &'a Option<SourceType>,
    conformance_conditions: &'a [(String, Expr)],
    where_clauses: &'a [Expr],
    fields: &'a [crate::ast::Param],
    associated: &'a [StructComptime],
    methods: &'a [Method],
    fieldwise_init: bool,
    decorators: &'a [crate::ast::Decorator],
}

/// View a statement as a struct declaration, for the order-independent
/// declaration pre-passes and the source-order walk alike.
fn struct_declaration(stmt: &Stmt) -> Option<StructDeclaration<'_>> {
    let StmtKind::Struct {
        name,
        type_params,
        conforms,
        callable_conformance,
        conformance_conditions,
        where_clauses,
        fields,
        associated,
        methods,
        fieldwise_init,
        decorators,
    } = &stmt.kind
    else {
        return None;
    };
    Some(StructDeclaration {
        module: &stmt.module,
        name,
        type_params,
        conforms,
        callable_conformance,
        conformance_conditions,
        where_clauses,
        fields,
        associated,
        methods,
        fieldwise_init: *fieldwise_init,
        decorators,
    })
}

/// Compose inherited associated-member requirements. Type-valued members with
/// the same name denote one associated type, so refinement accumulates their
/// bounds instead of treating stronger composition as an ambiguity. Value
/// members must retain one exact type; mixing value and type requirements is a
/// real conflict.
fn merge_associated_requirement(
    existing: &mut CtMemberReq,
    incoming: &CtMemberReq,
    member: &str,
) -> Result<(), TypeError> {
    match (existing, incoming) {
        (
            CtMemberReq::Type { bounds, params },
            CtMemberReq::Type {
                bounds: more,
                params: more_params,
            },
        ) => {
            // A refined associated type must keep the same parameterization.
            if !params.is_empty() && !more_params.is_empty() && params != more_params {
                return Err(TypeError::Unsupported(format!(
                    "refined associated type '{member}' changes its parameter list"
                )));
            }
            if params.is_empty() {
                *params = more_params.clone();
            }
            for bound in more {
                if !bounds.contains(bound) {
                    bounds.push(bound.clone());
                }
            }
            Ok(())
        }
        (CtMemberReq::Value(left), CtMemberReq::Value(right)) if left == right => Ok(()),
        _ => Err(TypeError::Unsupported(format!(
            "conflicting inherited associated member '{member}'"
        ))),
    }
}

fn conformance_operand(expression: &Expr, arguments: &HashMap<&str, &TyArg>) -> Option<CtValue> {
    match &expression.kind {
        ExprKind::Int(value) => Some(CtValue::IntLiteral(value.clone())),
        ExprKind::Bool(value) => Some(CtValue::Bool(*value)),
        ExprKind::Str(value) => Some(CtValue::Str(value.clone())),
        ExprKind::Identifier(name) => match arguments.get(name.as_str())? {
            TyArg::Val(value) => Some((*value).clone()),
            TyArg::Ty(_) | TyArg::Origin(_) => None,
        },
        _ => None,
    }
}

fn compare_ct_integers(op: InfixOp, left: &CtValue, right: &CtValue) -> Option<bool> {
    let (left, right) = (ct_integer(left)?, ct_integer(right)?);
    Some(match op {
        InfixOp::Eq => left == right,
        InfixOp::Ne => left != right,
        InfixOp::Lt => left < right,
        InfixOp::Le => left <= right,
        InfixOp::Gt => left > right,
        InfixOp::Ge => left >= right,
        _ => return None,
    })
}

fn ty_args_equal(left: &TyArg, right: &TyArg) -> bool {
    match (left, right) {
        (TyArg::Val(left), TyArg::Val(right)) => ct_values_equal(left, right),
        _ => left == right,
    }
}

type MethodInstantiation = (
    Vec<Ty>,
    Option<Ty>,
    Option<Ty>,
    HashMap<String, Ty>,
    HashMap<String, TyArg>,
);

fn same_method_shape(a: &MethodSig, b: &MethodSig) -> bool {
    // Keyword-only parameter NAMES are part of overload identity: two
    // signatures with identical types may still be distinct overloads when
    // their keyword-only selectors differ (`s[byte=i]` vs `s[codepoint=i]`).
    let keyword_names = |sig: &MethodSig| match sig.keyword_only {
        Some(index) => sig.names[index..].to_vec(),
        None => Vec::new(),
    };
    method_arity_range(a) == method_arity_range(b)
        && symbol_equivalent_params(&a.params, &b.params)
        && a.variadic == b.variadic
        && a.kw_variadic == b.kw_variadic
        && keyword_names(a) == keyword_names(b)
}

/// Current Mojo rejects a `__setitem__` pair whose assignment value is the
/// final positional parameter in one overload and a keyword-only parameter in
/// the other over the same index types: selection would otherwise depend on
/// the assignment's right-hand side.
fn competing_setitem_value_shapes(a: &MethodSig, b: &MethodSig) -> bool {
    fn positional_value_indices(sig: &MethodSig) -> Option<&[Ty]> {
        (sig.keyword_only.is_none()
            && sig.variadic.is_none()
            && sig.kw_variadic.is_none()
            && !sig.params.is_empty())
        .then(|| &sig.params[..sig.params.len() - 1])
    }
    fn keyword_value_indices(sig: &MethodSig) -> Option<&[Ty]> {
        let keyword_only = sig.keyword_only?;
        (sig.variadic.is_none() && sig.kw_variadic.is_none() && sig.names.len() == keyword_only + 1)
            .then(|| &sig.params[..keyword_only])
    }
    fn competes(positional: &MethodSig, keyword: &MethodSig) -> bool {
        matches!(
            (
                positional_value_indices(positional),
                keyword_value_indices(keyword),
            ),
            (Some(left), Some(right)) if symbol_equivalent_params(left, right)
        )
    }
    competes(a, b) || competes(b, a)
}

/// A conforming method may promise no error where its trait requirement raises,
/// but a raising implementation must preserve the exact declared error family.
/// Bare `raises` denotes `Error`; it is not a wildcard for a distinct typed
/// error. `raises Never` is already normalized to a non-raising signature when
/// `MethodSig` is built.
fn method_satisfies_requirement(got: &MethodSig, required: &MethodSig) -> bool {
    let mut got_shape = got.clone();
    got_shape.raises = false;
    got_shape.error = None;
    let mut required_shape = required.clone();
    required_shape.raises = false;
    required_shape.error = None;
    if got_shape != required_shape {
        return false;
    }
    if !got.raises {
        return true;
    }
    if !required.raises {
        return false;
    }
    got.error == required.error
}

fn method_callable_ty(method: &MethodSig) -> Ty {
    Ty::Func {
        environment: crate::origin::CallableEnvironment::Default,
        params: method.params.clone(),
        names: method.names.clone(),
        ret: Box::new(method.ret.clone()),
        required: method.required.clone(),
        variadic: method.variadic.clone(),
        kw_variadic: method.kw_variadic.clone(),
        positional_only: method.positional_only,
        keyword_only: method.keyword_only,
        raises: method.raises,
        error: method.error.clone(),
        conventions: method.conventions.clone(),
        ref_params: Box::new(method.ref_params.clone()),
        ref_return: method.ref_return.clone().map(Box::new),
        transfers: Default::default(),
    }
}

/// Merge a callable's committed transfer effects into a function type taken
/// as a value, so an indirect call replays them from the type itself. Union,
/// never replacement: a rebake after the entry grew keeps earlier effects.
fn with_transfer_effects(mut callable: Ty, effects: &[crate::checked::TransferEffect]) -> Ty {
    match &mut callable {
        Ty::Func { transfers, .. } | Ty::GenericFunc { transfers, .. } => {
            for effect in effects {
                if !transfers.0.contains(effect) {
                    transfers.0.push(effect.clone());
                }
            }
        }
        _ => {}
    }
    callable
}

/// The transfer effects carried by a callable value's checked type (empty
/// for non-callables and for contracts that never had effects baked).
fn contract_transfer_effects(ty: &Ty) -> &[crate::checked::TransferEffect] {
    match callable_contract_ty(ty) {
        Some(Ty::Func { transfers, .. }) | Some(Ty::GenericFunc { transfers, .. }) => &transfers.0,
        _ => &[],
    }
}

fn with_callable_environment(
    mut callable: Ty,
    environment: crate::origin::CallableEnvironment,
) -> Ty {
    match &mut callable {
        Ty::Func {
            environment: current,
            ..
        }
        | Ty::GenericFunc {
            environment: current,
            ..
        } => *current = environment,
        _ => {}
    }
    callable
}

fn overload_rank(conversions: usize, variadic: bool, signature_len: usize, generic: bool) -> usize {
    conversions * CONVERSION_RANK
        + usize::from(variadic) * VARIADIC_RANK
        + signature_len * SIGNATURE_LENGTH_RANK
        + usize::from(generic)
}

fn conversion_count(actual: &Ty, expected: &Ty) -> usize {
    if actual == expected
        || matches!(actual, Ty::IntLiteral) && matches!(expected, Ty::Int)
        || matches!(actual, Ty::FloatLiteral) && matches!(expected, Ty::Float64)
    {
        0
    } else {
        1
    }
}

struct MethodCallScore {
    rank: usize,
    slots: Vec<ArgSlot>,
    positional_overflow: Vec<usize>,
    keyword_overflow: Vec<usize>,
}

/// Interior-generation state immediately before applying one selected method
/// contract. Candidate scoring has already inferred the argument expressions at
/// this point, so subtracting this snapshot isolates callee effects from effects
/// which belong to evaluation of the source expression itself.
struct CallBoundarySnapshot {
    invalidations: HashMap<SourceSpan, Vec<crate::checked::InteriorInvalidation>>,
}

#[derive(Clone)]
struct ValueAdjustmentSnapshot {
    source: SourceSpan,
    overload_target: Option<String>,
    implicit_conversion: Option<String>,
    operation: Option<crate::checked::SemanticAdjustment>,
}

struct SubscriptResolution {
    return_type: Ty,
    lowered_name: Option<String>,
    value_keyword: bool,
}

fn select_callable_overload(
    matches: Vec<(Ty, usize, String, Option<Ty>)>,
) -> Result<(Ty, String, Option<Ty>), OverloadSelect> {
    let best = matches
        .iter()
        .map(|(_, score, _, _)| *score)
        .min()
        .ok_or(OverloadSelect::NoMatch)?;
    let mut best_matches = matches
        .into_iter()
        .filter(|(_, score, _, _)| *score == best)
        .collect::<Vec<_>>();
    if best_matches.len() != 1 {
        return Err(OverloadSelect::Ambiguous);
    }
    let (ret, _, target, error) = best_matches.remove(0);
    Ok((ret, target, error))
}

fn select_method_overload(
    _method: &str,
    matches: Vec<MethodCallResolution>,
    receiver_transferred: Option<bool>,
) -> Result<MethodCallResolution, OverloadSelect> {
    let best = matches
        .iter()
        .map(|candidate| candidate.conversion_score)
        .min()
        .ok_or(OverloadSelect::NoMatch)?;
    let mut best_matches = matches
        .into_iter()
        .filter(|candidate| candidate.conversion_score == best)
        .collect::<Vec<_>>();
    if best_matches.len() == 1 {
        return Ok(best_matches.remove(0));
    }
    // Current Mojo overloads some methods purely on the receiver convention
    // (a consuming `x^.m()` beside a borrowing `x.m()` — see
    // `symbol::receiver_overloaded_method`). Argument scoring cannot separate
    // those, so an otherwise tied set falls back to the call's explicit
    // receiver transfer. Only an exact single survivor resolves; anything
    // else stays ambiguous. The implicitly-copyable place satisfying a sole
    // deinit overload is unaffected — that path never reaches a tie.
    if let Some(transferred) = receiver_transferred {
        let survivors: Vec<usize> = best_matches
            .iter()
            .enumerate()
            .filter(|(_, candidate)| candidate.consumes_receiver == transferred)
            .map(|(index, _)| index)
            .collect();
        if let [index] = survivors.as_slice() {
            return Ok(best_matches.remove(*index));
        }
    }
    Err(OverloadSelect::Ambiguous)
}

fn overload_candidates(existing: &Ty, new_ty: &Ty) -> Option<Vec<Ty>> {
    callable_parameter_count(new_ty)?;
    match existing {
        Ty::Func { .. } | Ty::GenericFunc { .. }
            if callable_parameter_count(existing).is_some() =>
        {
            Some(vec![existing.clone()])
        }
        Ty::Overload(candidates) => Some(candidates.clone()),
        _ => None,
    }
}

type SplitCallableSpecialization = (
    Vec<crate::ast::ParamArg>,
    Vec<(Vec<usize>, crate::origin::Origin)>,
);

/// Mojo's built-in traits that mojito recognizes in a type-parameter bound.
/// User-defined traits (and conformance checking) are a later phase, so a bound
/// must name one of these. `AnyType` is the least restrictive.
const BUILTIN_TRAITS: &[&str] = &[
    "AnyType",
    "Deinitable",
    "Movable",
    "Copyable",
    "ImplicitlyCopyable",
    "RegisterPassable",
    "TrivialRegisterPassable",
    "Defaultable",
    "Representable",
    "Writable",
    "Writer",
    "Boolable",
    "Intable",
    "Floatable",
    "Indexer",
    "Equatable",
    "Comparable",
    "Hashable",
    "Hasher",
    "Identifiable",
    "Sized",
    "SizedRaising",
    "Iterable",
    "IterableOwned",
    "Iterator",
    "Absable",
    "Powable",
    "Roundable",
    "Ceilable",
    "Floorable",
    "Truncable",
    "CeilDivable",
    "CeilDivableRaising",
    "DivModable",
    "Addable",
    "Subtractable",
    "Multipliable",
    "Divisible",
    "FloorDivisible",
    "Modable",
    "ShiftLeftable",
    "ShiftRightable",
    "Andable",
    "Orable",
    "Xorable",
    "Negatable",
];

/// The linker qualifies `from std.utils import Variant` declarations.  Keep the
/// intrinsic recognition narrow so an unrelated user type ending in `Variant`
/// does not silently acquire built-in semantics.
fn is_variant_name(name: &str) -> bool {
    matches!(
        name,
        "Variant" | "__module$std$utilsVariant" | "__module$std$utils$Variant"
    )
}

mod places;

use places::*;

mod generics;

use generics::*;

mod declarations;

use declarations::*;

mod annotations;

use annotations::*;

mod calls;

use calls::*;

mod builtins;

pub(crate) use builtins::{builtin_copy_is_value_read, callable_environment_coerces};

use builtins::*;

mod operators;

use operators::*;

mod iteration;

mod type_resolution;

mod constraints;

mod scopes;

mod origins;

use origins::*;

mod traits;

mod inference;

mod indexing;

mod method_calls;

mod call_inference;

mod statements;

#[cfg(test)]
mod dependent_callable_signature_tests {
    use super::*;

    fn indexed_callable(binder: &str, offset: i64) -> Ty {
        let index = if offset == 0 {
            CtExpr::Param(binder.to_string())
        } else {
            CtExpr::Add(
                Box::new(CtExpr::Param(binder.to_string())),
                Box::new(CtExpr::Value(CtValue::Int(offset))),
            )
        };
        Ty::GenericFunc {
            environment: crate::origin::CallableEnvironment::Thin,
            decls: vec![ParamDecl::Value {
                name: binder.to_string(),
                ty: Box::new(Ty::Int),
                default: None,
                callable_default: None,
                infer_only: false,
                variadic: false,
                constraints: Vec::new(),
            }],
            params: vec![Ty::Dependent(DependentType::Indexed {
                elements: vec![Ty::Int, Ty::StringLiteral],
                index,
            })],
            names: vec!["element".to_string()],
            ret: Box::new(Ty::None),
            required: vec![true],
            variadic: None,
            kw_variadic: None,
            positional_only: None,
            keyword_only: None,
            raises: false,
            error: None,
            conventions: vec![Some(ArgConvention::Var)],
            ref_params: Box::new(vec![None]),
            ref_return: None,
            transfers: Default::default(),
        }
    }

    fn callable_with_parameter_role(role: crate::ast::ParamKind) -> Ty {
        let mut callable = indexed_callable("index", 0);
        let Ty::GenericFunc {
            params,
            names,
            required,
            variadic,
            kw_variadic,
            conventions,
            ref_params,
            ..
        } = &mut callable
        else {
            unreachable!("indexed_callable constructs a generic function")
        };
        let parameter = params.pop().expect("one regular parameter");
        names.clear();
        required.clear();
        conventions.clear();
        ref_params.clear();
        match role {
            crate::ast::ParamKind::Regular => params.push(parameter),
            crate::ast::ParamKind::Variadic => *variadic = Some(Box::new(parameter)),
            crate::ast::ParamKind::KwVariadic => *kw_variadic = Some(Box::new(parameter)),
        }
        callable
    }

    #[test]
    fn dependent_callable_binders_are_alpha_equivalent() {
        assert!(same_callable_signature(
            &indexed_callable("index", 0),
            &indexed_callable("i", 0),
        ));
        assert!(!same_callable_signature(
            &indexed_callable("index", 0),
            &indexed_callable("i", 1),
        ));
    }

    #[test]
    fn generic_parameter_roles_are_not_redeclaration_equivalent() {
        let regular = callable_with_parameter_role(crate::ast::ParamKind::Regular);
        let variadic = callable_with_parameter_role(crate::ast::ParamKind::Variadic);
        let kw_variadic = callable_with_parameter_role(crate::ast::ParamKind::KwVariadic);
        assert!(!same_callable_signature(&regular, &variadic));
        assert!(!same_callable_signature(&regular, &kw_variadic));
        assert!(!same_callable_signature(&variadic, &kw_variadic));
    }
}

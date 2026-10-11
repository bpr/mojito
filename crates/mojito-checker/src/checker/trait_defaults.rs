//! Trait defaults as upstream emits them: each default is its trait's member
//! `Trait.method`, one function over the trait's `Self`, checked and lowered
//! once, and a conformer that does not spell a defaulted method gets a member
//! over the default's resolved signature whose body is one call of the
//! trait's member (`StructEmitter::synthesizeDefaultTraitMethodWrapper` and
//! `populateDefaultedTraitFunction`). The elaborator elaborates a default
//! inside its trait; the check checks the member after every module
//! declaration is bound, and builds the wrapper.

use mojito_types::types::TyKind;
use std::collections::{HashMap, HashSet};

use mojito_ast::ast::{
    ArgConvention, DeclaredParam, Expr, ExprKind, FnParam, KwArg, ParamArg, ParamKind, Stmt,
    StmtKind, TraitMethod, Type, TypeParam,
};
use mojito_ast::visit::{MutVisitor, walk_block_mut, walk_stmt_mut};
use mojito_checked::checked::{CheckedDefaultWrapper, CheckedTraitMember, SignatureParam};
use mojito_common::error::TypeError;
use mojito_common::literal::IntLiteral;
use mojito_common::token::{Span, SyntaxId};
use mojito_symbol::symbol::OverloadLookup;
use mojito_symbol::symbol::SignatureKey;

use super::declarations::{BodyParam, MethodBody};
use super::origins::ref_binding_is_writable;
use super::overload_support::method_lowered_name;
use super::traits_support::same_method_shape;
use super::{Checker, MethodSig, StructDeclaration, Ty};
use mojito_symbol::symbol::lifecycle_method_name;

/// The declaration trait default `index` of `trait_stmt` is checked and
/// lowered as: its trait's member, named `name`.
///
/// Its first compile-time parameter is the trait's `Self` (spelled `_Self`,
/// as upstream's diagnostics name it), its first argument the receiver, and
/// its body the default's. `None` when the method declares no default.
///
/// Its syntax is derived from the trait statement's, so the check, the
/// elaborator, and MIR each build the same declaration.
pub fn trait_member_declaration(trait_stmt: &Stmt, index: usize, name: String) -> Option<Stmt> {
    let StmtKind::Trait {
        name: trait_name,
        methods,
        ..
    } = &trait_stmt.kind
    else {
        return None;
    };
    let method = methods.get(index)?;
    let body = method.default_body.clone()?;
    let self_param = TypeParam {
        name: SELF_PARAM.to_string(),
        bounds: vec![trait_name.clone()],
        value_type: None,
        callable_bound: None,
        origin_mutability: None,
        // The receiver's type infers it, so it joins an infer-only section
        // the default's own parameters open.
        infer_only: method
            .type_params
            .first()
            .is_some_and(|param| param.infer_only),
        default: None,
        constraints: Vec::new(),
    };
    let receiver = FnParam {
        name: "self".to_string(),
        ty: Type::Named(SELF_PARAM.to_string(), Vec::new()),
        default: None,
        kind: ParamKind::Regular,
        convention: method.self_convention,
        origin: method.self_origin.clone(),
    };
    let mut function = Stmt {
        kind: StmtKind::Def {
            name,
            decorators: Vec::new(),
            type_params: std::iter::once(self_param)
                .chain(method.type_params.iter().cloned())
                .collect(),
            params: std::iter::once(receiver)
                .chain(method.params.iter().cloned())
                .collect(),
            positional_only: method.positional_only.map(|index| index + 1),
            keyword_only: method.keyword_only.map(|index| index + 1),
            captures: None,
            raises: method.raises,
            raises_type: method.raises_type.clone(),
            ret: method.ret.clone(),
            where_clauses: method.where_clauses.clone(),
            body,
        },
        span: trait_stmt.span,
        module: trait_stmt.module.clone(),
        syntax_id: SyntaxId::derived(
            trait_stmt.syntax_id,
            u32::try_from(index).unwrap_or(u32::MAX),
        ),
    };
    walk_stmt_mut(&mut SpellSelfParam, &mut function);
    if let StmtKind::Def { type_params, .. } = &mut function.kind {
        for param in type_params {
            for bound in &mut param.bounds {
                if let Some(rest) = bound.strip_prefix("Self.") {
                    *bound = format!("{SELF_PARAM}.{rest}");
                }
            }
        }
    }
    Some(function)
}

/// A trait's own default, as its declaration gives it: what a conformer
/// that inherits it binds and forwards, whichever program declared the
/// trait.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct DefaultShape {
    pub(super) method: String,
    /// Its trait's member symbol, `Trait.method`, signature-qualified as the
    /// requirement's binder owner is when the name is overloaded.
    pub(super) member: String,
    key: OverloadKey,
    /// Its requirement's position among its trait's same-name requirement
    /// signatures, which the trait's check records ([`Self::at_overload`]).
    overload: usize,
    /// Its declared parameter count, as the overload prescan counts it.
    pub(super) arity: usize,
    /// Its own compile-time parameters' names, a pack's without its `*`.
    binders: Vec<String>,
    /// Every declared parameter but the receiver, in order. Their defaults
    /// are the member's, which its declaration holds.
    pub(super) params: Vec<SignatureParam>,
    receiver: Option<ArgConvention>,
    /// Whether it declares a result, which the forwarding call returns.
    returns: bool,
    /// Whether it declares `raises`.
    raises: bool,
    /// Whether the forwarding call binds `Self` explicitly: unless the
    /// default opens an infer-only section, which the receiver infers.
    explicit_self: bool,
    /// The compile-time parameters the forwarding call binds by name.
    forwarded: Vec<String>,
    /// Why no wrapper can forward to it, when none can.
    unforwardable: Option<String>,
}

impl DefaultShape {
    /// This default, its requirement the `overload`-th signature of its name.
    pub(super) fn at_overload(self, overload: usize) -> Self {
        Self { overload, ..self }
    }
}

/// What a trait gives a conformer: its refinements, and its own defaults.
pub(super) type TraitDefaults = (Vec<String>, Vec<DefaultShape>);

/// A trait default the check checked as its trait's member.
pub(super) struct TraitMember {
    module: Option<String>,
    trait_name: String,
    /// Its position among its trait's methods.
    method_index: usize,
    /// Its member symbol ([`member_symbol`]).
    lowered_name: String,
}

/// The wrapper of one default a struct inherits, before the check
/// registers its signature.
#[derive(Clone)]
pub(super) struct DefaultWrapper {
    pub(super) module: Option<String>,
    /// The struct that inherits the default.
    pub(super) declaration: String,
    pub(super) trait_name: String,
    pub(super) shape: DefaultShape,
    /// Its member position: after the struct's spelled methods, in the order
    /// its defaults are reached.
    pub(super) method_index: usize,
    pub(super) body: Vec<Stmt>,
    /// The symbol it lowers to, which registration qualifies when the name
    /// is overloaded.
    pub(super) lowered_name: String,
}

impl DefaultWrapper {
    /// Whether struct `declaration` is the one that inherits it.
    fn inherited_by(&self, declaration: &StructDeclaration<'_>) -> bool {
        (self.declaration.as_str(), &self.module) == (declaration.name, declaration.module)
    }
}

/// The defaults `methods` of trait `trait_name` declare, in order.
pub(super) fn default_shapes(
    trait_name: &str,
    methods: &[TraitMethod],
    sets: &impl OverloadLookup,
    constants: &dyn Fn(&str) -> Option<IntLiteral>,
) -> Vec<DefaultShape> {
    methods
        .iter()
        .filter(|method| method.default_body.is_some())
        .map(|method| DefaultShape {
            method: method.name.clone(),
            member: member_symbol(trait_name, method, sets, constants),
            key: overload_key(&method.name, &method.params, method.keyword_only, None),
            overload: 0,
            arity: method.params.len(),
            binders: method
                .type_params
                .iter()
                .map(|param| param_name(param).to_string())
                .collect(),
            params: method
                .params
                .iter()
                .map(|param| SignatureParam {
                    name: param.name.clone(),
                    kind: param.kind,
                    convention: param.convention,
                })
                .collect(),
            receiver: method.self_convention,
            returns: method.ret.is_some(),
            raises: method.raises,
            explicit_self: !method
                .type_params
                .first()
                .is_some_and(|param| param.infer_only),
            forwarded: method
                .type_params
                .iter()
                .filter(|param| {
                    !param.infer_only
                        && !param.name.starts_with('*')
                        && !param.is_erased_origin_param(&method.type_params)
                })
                .map(|param| param.name.clone())
                .collect(),
            unforwardable: unforwardable(method),
        })
        .collect()
}

/// The defaults struct `stmt` inherits and does not spell, each with the
/// trait that declares it: its conformances' refinements first, each
/// default once however many paths reach it. `traits` gives a trait's
/// refinements and its own defaults.
pub(super) fn inherited_defaults(
    stmt: &Stmt,
    traits: &dyn Fn(&str) -> Option<TraitDefaults>,
) -> Result<Vec<(String, DefaultShape)>, TypeError> {
    let StmtKind::Struct {
        name,
        type_params,
        conforms,
        methods,
        ..
    } = &stmt.kind
    else {
        return Ok(Vec::new());
    };
    let explicit: Vec<_> = methods
        .iter()
        .map(|method| {
            overload_key(
                &method.name,
                &method.params,
                method.keyword_only,
                Some(name),
            )
        })
        .collect();
    let mut reached: Vec<(String, DefaultShape)> = Vec::new();
    for trait_name in conforms {
        defaults_for(trait_name, traits, &mut HashSet::new(), &mut reached)?;
    }
    let mut inherited: Vec<(String, DefaultShape)> = Vec::new();
    for (trait_name, default) in reached {
        if explicit.contains(&default.key) {
            continue;
        }
        if let Some((original, _)) = inherited.iter().find(|(_, seen)| seen.key == default.key) {
            return Err(TypeError::ConflictingTraitDefaults {
                method: default.method,
                original: original.clone(),
                conflicting: trait_name,
            });
        }
        if let Some(conflict) = default.binders.iter().find(|binder| {
            type_params
                .iter()
                .any(|own| param_name(own) == binder.as_str())
        }) {
            return Err(TypeError::TraitDefaultParameterConflict(conflict.clone()));
        }
        inherited.push((trait_name, default));
    }
    Ok(inherited)
}

/// The wrapper of every default struct `stmt` inherits, as
/// [`inherited_defaults`] reaches them, each body one statement calling the
/// default's trait member with `Self` bound first and every argument forwarded,
/// as `populateDefaultedTraitFunction` forwards it.
///
/// Each wrapper's syntax is derived from the struct statement's identity,
/// so every check of the program builds the same occurrences.
pub(super) fn default_wrappers(
    stmt: &Stmt,
    traits: &dyn Fn(&str) -> Option<TraitDefaults>,
) -> Result<Vec<DefaultWrapper>, TypeError> {
    let StmtKind::Struct { name, methods, .. } = &stmt.kind else {
        return Ok(Vec::new());
    };
    let mut ids = DeriveSyntax {
        parent: stmt.syntax_id,
        next: 0,
    };
    inherited_defaults(stmt, traits)?
        .into_iter()
        .enumerate()
        .map(|(index, (trait_name, shape))| {
            if let Some(reason) = &shape.unforwardable {
                return Err(TypeError::Unsupported(reason.clone()));
            }
            let mut body = vec![forwarding_statement(&shape, stmt.span)];
            if let Some(module) = &stmt.module {
                mojito_ast::ast::stamp_source(&mut body, module);
            }
            walk_block_mut(&mut ids, &mut body);
            Ok(DefaultWrapper {
                module: stmt.module.clone(),
                declaration: name.clone(),
                lowered_name: format!("{name}.{}", shape.method),
                trait_name,
                shape,
                method_index: methods.len() + index,
                body,
            })
        })
        .collect()
}

impl Checker {
    /// The wrapper of every default a top-level struct of `stmts` inherits,
    /// each counted among its struct's same-name methods for the overload
    /// prescan. An own trait's defaults are read from its declaration, a
    /// package trait's from its installed record.
    pub(super) fn plan_default_wrappers(
        &mut self,
        stmts: &[Stmt],
    ) -> Result<Vec<DefaultWrapper>, TypeError> {
        let own: HashMap<&str, TraitDefaults> = stmts
            .iter()
            .filter_map(|stmt| match &stmt.kind {
                StmtKind::Trait {
                    name,
                    refines,
                    methods,
                    ..
                } => Some((
                    name.as_str(),
                    (
                        refines.clone(),
                        default_shapes(name, methods, &*self.overload_sets, &|name| {
                            self.program_constants.get(name)
                        }),
                    ),
                )),
                _ => None,
            })
            .collect();
        let lookup = |name: &str| {
            own.get(name).cloned().or_else(|| {
                self.traits
                    .get(name)
                    .map(|info| (info.refines.clone(), info.defaults.clone()))
            })
        };
        let mut wrappers = Vec::new();
        let mut declared = Vec::new();
        for stmt in stmts {
            let StmtKind::Struct { name, methods, .. } = &stmt.kind else {
                continue;
            };
            let found = default_wrappers(stmt, &lookup)?;
            let names: HashSet<&str> = found
                .iter()
                .map(|wrapper| wrapper.shape.method.as_str())
                .collect();
            for method in names {
                let arities = methods
                    .iter()
                    .filter(|spelled| lifecycle_method_name(spelled) == method)
                    .map(|spelled| spelled.params.len())
                    .chain(
                        found
                            .iter()
                            .filter(|wrapper| wrapper.shape.method == method)
                            .map(|wrapper| wrapper.shape.arity),
                    )
                    .collect();
                declared.push((format!("{name}.{method}"), arities));
            }
            wrappers.extend(found);
        }
        for (source, arities) in declared {
            std::sync::Arc::make_mut(&mut self.overload_sets).declare_method(&source, arities);
        }
        Ok(wrappers)
    }

    /// Register the signature of every wrapper struct `declaration`
    /// inherits, after its spelled methods: the default's resolved
    /// signature with `Self` the struct and its binders owned by the
    /// wrapper, as `synthesizeDefaultTraitMethodWrapper` builds the
    /// wrapper's `FnOp` from `getTraitFunctionSignature`.
    pub(super) fn register_default_wrappers(
        &mut self,
        declaration: &StructDeclaration<'_>,
    ) -> Result<(), TypeError> {
        for slot in 0..self.default_wrappers.len() {
            let wrapper = &self.default_wrappers[slot];
            if !wrapper.inherited_by(declaration) {
                continue;
            }
            let wrapper = wrapper.clone();
            let lowered = self.register_default_wrapper(declaration, &wrapper)?;
            self.default_wrappers[slot].lowered_name = lowered;
        }
        Ok(())
    }

    /// The named destructors struct `declaration` inherits: each wrapper of
    /// a default with a `deinit` receiver, and whether it raises.
    pub(super) fn inherited_destructors(
        &self,
        declaration: &StructDeclaration<'_>,
    ) -> Vec<(String, bool)> {
        self.default_wrappers
            .iter()
            .filter(|wrapper| {
                wrapper.inherited_by(declaration)
                    && wrapper.shape.method != "__deinit__"
                    && wrapper.shape.receiver == Some(ArgConvention::Deinit)
            })
            .map(|wrapper| (wrapper.shape.method.clone(), wrapper.shape.raises))
            .collect()
    }

    /// Check every default a top-level trait of `stmts` declares as its
    /// trait's member, once every module declaration is bound, as upstream
    /// emits a default inside its trait: its body names any function of the
    /// module, wherever declared. The member's callable binds under its
    /// symbol, which only a conformer's wrapper calls.
    pub(super) fn check_trait_members(&mut self, stmts: &[Stmt]) -> Result<(), TypeError> {
        for stmt in stmts {
            let StmtKind::Trait { name, methods, .. } = &stmt.kind else {
                continue;
            };
            for (index, method) in methods.iter().enumerate() {
                let symbol = member_symbol(name, method, &*self.overload_sets, &|name| {
                    self.program_constants.get(name)
                });
                let Some(declaration) = trait_member_declaration(stmt, index, symbol.clone())
                else {
                    continue;
                };
                self.checking_trait_member = true;
                let checked = self.check_def(&declaration, false);
                self.checking_trait_member = false;
                checked?;
                self.trait_members.push(TraitMember {
                    module: stmt.module.clone(),
                    trait_name: name.clone(),
                    method_index: index,
                    lowered_name: symbol,
                });
            }
        }
        Ok(())
    }

    /// Check the body of every wrapper a top-level struct of `stmts`
    /// inherits, after every trait member its wrappers call is bound, each
    /// the next of its name's overloads after the struct's spelled methods.
    pub(super) fn check_default_wrappers(&mut self, stmts: &[Stmt]) -> Result<(), TypeError> {
        for stmt in stmts {
            let Some(declaration) = super::overload_support::struct_declaration(stmt) else {
                continue;
            };
            let wrappers: Vec<DefaultWrapper> = self
                .default_wrappers
                .iter()
                .filter(|wrapper| wrapper.inherited_by(&declaration))
                .cloned()
                .collect();
            if wrappers.is_empty() {
                continue;
            }
            let mut overload_indices = HashMap::<String, usize>::new();
            for method in declaration.methods {
                *overload_indices
                    .entry(lifecycle_method_name(method).to_string())
                    .or_default() += 1;
            }
            let (self_ty, saved) = self.enter_struct_scope(&declaration)?;
            let checked = wrappers.iter().try_for_each(|wrapper| {
                let overload_index = overload_indices
                    .entry(wrapper.shape.method.clone())
                    .or_default();
                let index = *overload_index;
                *overload_index += 1;
                self.check_default_wrapper(&self_ty, wrapper, index)
            });
            self.exit_struct_scope(saved);
            checked?;
        }
        Ok(())
    }

    /// The checked handoff of every trait member, each declaration built
    /// from its trait statement in `statements`, the checked tree.
    pub(super) fn checked_trait_members(&self, statements: &[Stmt]) -> Vec<CheckedTraitMember> {
        self.trait_members
            .iter()
            .filter_map(|member| {
                let trait_stmt = statements.iter().find(|stmt| {
                    stmt.module == member.module
                        && matches!(&stmt.kind, StmtKind::Trait { name, .. } if *name == member.trait_name)
                })?;
                Some(CheckedTraitMember {
                    module: member.module.clone(),
                    trait_name: member.trait_name.clone(),
                    method_index: member.method_index,
                    declaration: trait_member_declaration(
                        trait_stmt,
                        member.method_index,
                        member.lowered_name.clone(),
                    )?,
                })
            })
            .collect()
    }

    /// The checked handoff of every wrapper.
    pub(super) fn checked_default_wrappers(&self) -> Vec<CheckedDefaultWrapper> {
        self.default_wrappers
            .iter()
            .map(|wrapper| CheckedDefaultWrapper {
                module: wrapper.module.clone(),
                declaration: wrapper.declaration.clone(),
                method: wrapper.shape.method.clone(),
                method_index: wrapper.method_index,
                lowered_name: wrapper.lowered_name.clone(),
                member: wrapper.shape.member.clone(),
                self_convention: wrapper.shape.receiver,
                params: wrapper.shape.params.clone(),
                body: wrapper.body.clone(),
            })
            .collect()
    }

    /// Register `wrapper`'s signature on struct `declaration`, returning the
    /// symbol it lowers to.
    fn register_default_wrapper(
        &mut self,
        declaration: &StructDeclaration<'_>,
        wrapper: &DefaultWrapper,
    ) -> Result<String, TypeError> {
        use mojito_checked::checked::{AnnotationSite, DeclarationEffect, GenericSite};
        let name = declaration.name;
        let method = wrapper.shape.method.as_str();
        let requirement = self
            .traits
            .get(&wrapper.trait_name)
            .and_then(|info| {
                let default = info
                    .defaults
                    .iter()
                    .find(|default| default.member == wrapper.shape.member)?;
                info.methods.get(&default.method)?.get(default.overload)
            })
            .cloned()
            .ok_or_else(|| {
                TypeError::InvariantViolation(format!(
                    "trait '{}' records no default '{method}'",
                    wrapper.trait_name
                ))
            })?;
        let self_ty = self.self_ty.clone().ok_or_else(|| {
            TypeError::InvariantViolation(format!("struct '{name}' is not in scope"))
        })?;
        let mut sig = self.requirement_at(&requirement, &self_ty);
        let source = format!("{name}.{method}");
        let overloaded = self
            .overload_sets
            .method_is_overloaded(&source, wrapper.shape.arity);
        let lowered = if overloaded {
            method_lowered_name(name, method, &sig, self.self_instance_ty(name).as_ref())
        } else {
            source
        };
        let (decls, bindings) = mojito_types::types::reown_generic_signature(
            &sig.decls,
            &super::annotations::binder_owner(&lowered),
        );
        let moved = |ty: &Ty| {
            mojito_types::types::replace_parameters(
                &mojito_types::param_expr::ParamContext::detached(),
                ty,
                &bindings,
                0,
            )
            .unwrap_or_else(|_| ty.clone())
        };
        sig.decls = decls;
        sig.params = sig.params.iter().map(moved).collect();
        for ty in [&mut sig.variadic, &mut sig.kw_variadic, &mut sig.error]
            .into_iter()
            .flatten()
        {
            *ty = moved(ty);
        }
        sig.ret = moved(&sig.ret);
        sig.overload = overloaded
            .then(|| mojito_symbol::symbol::overload_qualifier(&lowered).map(str::to_string))
            .flatten();
        let module = declaration.module.clone();
        let index = wrapper.method_index;
        self.generic_parameters.borrow_mut().insert(
            GenericSite::Method {
                module: module.clone(),
                declaration: name.to_string(),
                method: index,
            },
            sig.decls.clone(),
        );
        let mut regular = sig.params.iter();
        for (param, declared) in wrapper.shape.params.iter().enumerate() {
            let ty = match declared.kind {
                ParamKind::Regular => regular.next().cloned(),
                ParamKind::Variadic => sig.variadic.clone(),
                ParamKind::KwVariadic => sig.kw_variadic.clone(),
            }
            .ok_or_else(|| {
                TypeError::InvariantViolation(format!(
                    "parameter '{}' of trait default '{method}' has no resolved type",
                    declared.name
                ))
            })?;
            self.declaration_types.borrow_mut().insert(
                AnnotationSite::MethodParam {
                    module: module.clone(),
                    declaration: name.to_string(),
                    method: index,
                    param,
                },
                ty,
            );
        }
        let return_site = AnnotationSite::MethodReturn {
            module,
            declaration: name.to_string(),
            method: index,
        };
        self.declaration_types
            .borrow_mut()
            .insert(return_site.clone(), sig.ret.clone());
        self.declaration_effects.borrow_mut().insert(
            return_site,
            DeclarationEffect {
                raises: sig.raises,
                error: sig.raises.then(|| sig.error.clone()).flatten(),
                returns_reference: sig.ref_return.is_some(),
                param_writes: DeclarationEffect::param_writes(&sig.conventions, &sig.ref_params),
                availability: sig.availability.clone(),
            },
        );
        let info = self.structs.get_mut(name).ok_or_else(|| {
            TypeError::InvariantViolation(format!("struct '{name}' was not registered"))
        })?;
        let overloads = info.methods.entry(method.to_string()).or_default();
        if overloads
            .iter()
            .any(|existing| same_method_shape(existing, &sig))
        {
            return Err(TypeError::Redeclaration(method.to_string()));
        }
        overloads.push(sig);
        Ok(lowered)
    }

    /// Check `wrapper`'s forwarding body as the `overload_index`-th overload
    /// of its name: its parameters bound at the types its registration
    /// recorded, in the transfer frame its symbol names.
    fn check_default_wrapper(
        &mut self,
        self_ty: &Ty,
        wrapper: &DefaultWrapper,
        overload_index: usize,
    ) -> Result<(), TypeError> {
        let declaration = wrapper.declaration.as_str();
        let method = wrapper.shape.method.as_str();
        let signature = self
            .structs
            .get(declaration)
            .and_then(|info| info.methods.get(method))
            .and_then(|methods| methods.get(overload_index))
            .cloned()
            .ok_or_else(|| {
                TypeError::InvariantViolation(format!(
                    "method signature for '{declaration}.{method}' is not registered"
                ))
            })?;
        let effect_key = self.method_effect_key(declaration, method, overload_index)?;
        self.parametric_write_frames.borrow_mut().push(Vec::new());
        let saved_site = self
            .method_site
            .replace((wrapper.module.clone(), declaration.to_string()));
        self.push_param_scope(&signature.decls);
        let saved_struct_count = self
            .enclosing_struct_type_params
            .replace(self.enclosing_type_params.len());
        self.assume_propositions(Vec::new());
        self.assumed_conformances.push(HashSet::new());
        let result = self.check_default_wrapper_body(self_ty, wrapper, &signature, effect_key);
        self.assumed_conformances.pop();
        self.enclosing_struct_type_params.set(saved_struct_count);
        self.tparams.pop();
        self.method_site = saved_site;
        self.parametric_write_frames.borrow_mut().pop();
        result
    }

    fn check_default_wrapper_body(
        &mut self,
        self_ty: &Ty,
        wrapper: &DefaultWrapper,
        signature: &MethodSig,
        effect_key: String,
    ) -> Result<(), TypeError> {
        let method = wrapper.shape.method.as_str();
        self.push_scope();
        self.raising_context.push(signature.error.clone());
        let result = (|| {
            self.declare_value_parameters(&signature.decls)?;
            let self_writable = ref_binding_is_writable(signature.self_convention, None, &[]);
            self.bind_receiver(self_ty, self_writable)?;
            let mut regular = signature.params.iter();
            for param in &wrapper.shape.params {
                let pty = match param.kind {
                    ParamKind::Regular => regular.next().cloned(),
                    ParamKind::Variadic => signature.variadic.clone().map(|element| match element
                        .kind()
                    {
                        TyKind::RuntimePack(elements) => TyKind::Tuple(elements.clone()).into(),
                        _ => TyKind::VariadicPack(element).into(),
                    }),
                    ParamKind::KwVariadic => signature
                        .kw_variadic
                        .clone()
                        .map(|element| {
                            self.kwargs_collector_ty(
                                element,
                                &format!("keyword collector '{}'", param.name),
                            )
                        })
                        .transpose()?,
                }
                .ok_or_else(|| {
                    TypeError::InvariantViolation(format!(
                        "parameter '{}' of '{}.{method}' has no checked type",
                        param.name, wrapper.declaration
                    ))
                })?;
                self.bind_parameter(
                    BodyParam {
                        name: &param.name,
                        kind: param.kind,
                        convention: param.convention,
                    },
                    &pty,
                    ref_binding_is_writable(param.convention, None, &[]),
                    None,
                )?;
            }
            self.check_method_body(MethodBody {
                name: method,
                params: wrapper
                    .shape
                    .params
                    .iter()
                    .map(|param| BodyParam {
                        name: &param.name,
                        kind: param.kind,
                        convention: param.convention,
                    })
                    .collect(),
                self_writable,
                initializing: false,
                effect_key,
                method_decls: &signature.decls,
                ref_return: signature.ref_return.clone(),
                return_annotation: None,
                generic: !signature.decls.is_empty(),
                ret_ty: &signature.ret,
                body: &wrapper.body,
            })
        })();
        self.raising_context.pop();
        self.pop_scope();
        result
    }
}

/// The member symbol default `method` of trait `trait_name` lowers to: the
/// requirement's binder owner, `Trait.method`, signature-qualified when the
/// trait overloads the name. Named by declaration, so it is stable across
/// packages; source cannot spell a dotted callee, so no source name reaches
/// it.
fn member_symbol(
    trait_name: &str,
    method: &TraitMethod,
    sets: &impl OverloadLookup,
    constants: &dyn Fn(&str) -> Option<IntLiteral>,
) -> String {
    mojito_symbol::symbol::lowered_method_name(
        &format!(
            "{}.{}",
            super::annotations::binder_owner(trait_name),
            super::annotations::binder_owner(&method.name)
        ),
        &[],
        mojito_symbol::symbol::MethodShape {
            type_params: &method.type_params,
            params: &method.params,
            keyword_only: method.keyword_only,
            has_self: true,
            self_convention: method.self_convention,
        },
        sets,
        constants,
    )
}

/// A compile-time parameter's name without a pack's leading `*`.
fn param_name(param: &TypeParam) -> &str {
    param.name.trim_start_matches('*')
}

/// A default method's identity among its same-name overloads: its name and
/// its parameters' overload signature, with the conformer's own name read as
/// `Self` so a method spelling either suppresses the default.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct OverloadKey(String, SignatureKey);

fn overload_key(
    name: &str,
    params: &[FnParam],
    keyword_only: Option<usize>,
    conformer: Option<&str>,
) -> OverloadKey {
    let params: Vec<FnParam> = params
        .iter()
        .filter(|param| !param.is_named_result())
        .map(|param| {
            let mut param = param.clone();
            if matches!(&param.ty, Type::Named(named, args) if args.is_empty() && Some(named.as_str()) == conformer)
            {
                param.ty = Type::SelfType;
            }
            param
        })
        .collect();
    let collector = params
        .iter()
        .position(|param| param.kind == ParamKind::Variadic);
    let keyword_names = params
        .iter()
        .enumerate()
        .filter(|(index, param)| {
            param.kind == ParamKind::Regular
                && [keyword_only, collector]
                    .into_iter()
                    .flatten()
                    .any(|start| *index >= start)
        })
        .map(|(_, param)| param.name.clone())
        .collect();
    OverloadKey(
        name.to_string(),
        SignatureKey::from_ast_params(&params).with_keyword_names(keyword_names),
    )
}

/// Add to `reached` every default trait `name` gives a conformer, its
/// refinements' first, each once however many paths reach it.
fn defaults_for(
    name: &str,
    traits: &dyn Fn(&str) -> Option<TraitDefaults>,
    visiting: &mut HashSet<String>,
    reached: &mut Vec<(String, DefaultShape)>,
) -> Result<(), TypeError> {
    if !visiting.insert(name.to_string()) {
        return Err(TypeError::Unsupported(format!(
            "cyclic trait refinement involving '{name}'"
        )));
    }
    let Some((refines, defaults)) = traits(name) else {
        visiting.remove(name);
        return Ok(());
    };
    for parent in &refines {
        defaults_for(parent, traits, visiting, reached)?;
    }
    for default in defaults {
        if !reached
            .iter()
            .any(|(_, seen)| seen.member == default.member)
        {
            reached.push((name.to_string(), default));
        }
    }
    visiting.remove(name);
    Ok(())
}

/// Why no conformer's wrapper can forward to default `method`, when none
/// can.
fn unforwardable(method: &TraitMethod) -> Option<String> {
    let name = &method.name;
    if let Some(param) = method.params.iter().find(|param| {
        param.kind == ParamKind::Variadic
            && !matches!(&param.ty, Type::Named(pack, _) if pack.starts_with('*'))
    }) {
        return Some(format!(
            "forwarding the homogeneous variadic '*{}' of trait default '{name}' to a \
             conformer: a homogeneous variadic spread into a call is not supported yet",
            param.name
        ));
    }
    if method.params.iter().any(DeclaredParam::is_named_result) {
        return Some(format!(
            "inheriting trait default '{name}', which declares a named 'out' result: a \
             trait method's named result is not supported yet"
        ));
    }
    let names_origin = method.self_origin.is_some()
        || method.params.iter().any(|param| param.origin.is_some())
        || matches!(method.ret, Some(Type::Ref { .. }))
        || method
            .type_params
            .iter()
            .any(|param| param.is_erased_origin_param(&method.type_params));
    if names_origin {
        return Some(format!(
            "inheriting trait default '{name}', whose signature names an origin: a \
             conformer's wrapper over such a signature is not supported yet"
        ));
    }
    (!method.where_clauses.is_empty()).then(|| {
        format!(
            "inheriting trait default '{name}', which declares a 'where' clause: a \
             conformer's wrapper over a constrained signature is not supported yet"
        )
    })
}

/// The one statement of a wrapper for `shape`: a call of its trait's member, with
/// `Self` bound first and every argument forwarded with the convention it
/// declares.
fn forwarding_statement(shape: &DefaultShape, span: Span) -> Stmt {
    let expr = |kind| Expr::new(kind, span);
    let name = |name: &str| expr(ExprKind::Identifier(name.to_string()));
    let transfer = |param: &str| expr(ExprKind::Transfer(Box::new(name(param))));
    let owned = |convention: Option<ArgConvention>| {
        matches!(convention, Some(ArgConvention::Var | ArgConvention::Deinit))
    };
    let mut param_args = Vec::new();
    if shape.explicit_self {
        param_args.push(ParamArg::Type(Type::SelfType));
    }
    param_args.extend(shape.forwarded.iter().map(|param| ParamArg::Named {
        name: param.clone(),
        value: Box::new(ParamArg::Value(name(param))),
    }));
    let mut args = vec![if owned(shape.receiver) {
        transfer("self")
    } else {
        name("self")
    }];
    let mut kwargs = Vec::new();
    let mut keyword = false;
    for param in &shape.params {
        let value = if owned(param.convention) {
            transfer(&param.name)
        } else {
            name(&param.name)
        };
        match param.kind {
            ParamKind::Regular if keyword => kwargs.push(KwArg {
                name: param.name.clone(),
                value,
            }),
            ParamKind::Regular => args.push(value),
            ParamKind::Variadic => {
                args.push(expr(ExprKind::Spread(Box::new(value))));
                keyword = true;
            }
            ParamKind::KwVariadic => kwargs.push(KwArg {
                name: "**".to_string(),
                value: transfer(&param.name),
            }),
        }
    }
    let call = expr(ExprKind::Call {
        name: shape.member.clone(),
        param_args,
        args,
        kwargs,
    });
    let statement = if shape.returns {
        StmtKind::Return(Some(call))
    } else {
        StmtKind::Expr(call)
    };
    Stmt::new(statement, span)
}

/// Number every node it walks as the next derivation of `parent`.
struct DeriveSyntax {
    parent: SyntaxId,
    next: u32,
}

impl DeriveSyntax {
    fn id(&mut self) -> SyntaxId {
        let id = SyntaxId::derived(self.parent, self.next);
        self.next += 1;
        id
    }
}

impl MutVisitor for DeriveSyntax {
    fn visit_stmt_mut(&mut self, statement: &mut Stmt) {
        statement.syntax_id = self.id();
    }

    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        expr.syntax_id = self.id();
    }
}

/// The trait's `Self` as its member names it.
const SELF_PARAM: &str = "_Self";

/// Respell the trait's `Self` as its member's [`SELF_PARAM`].
struct SpellSelfParam;

impl MutVisitor for SpellSelfParam {
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        match &mut expr.kind {
            ExprKind::Identifier(name) | ExprKind::Call { name, .. } if name == "Self" => {
                *name = SELF_PARAM.to_string();
            }
            _ => {}
        }
    }

    fn visit_type_mut(&mut self, ty: &mut Type) {
        match ty {
            Type::SelfType => *ty = Type::Named(SELF_PARAM.to_string(), Vec::new()),
            Type::SelfParam(member) if !member.starts_with('*') => {
                *ty = Type::Assoc {
                    base: Box::new(Type::Named(SELF_PARAM.to_string(), Vec::new())),
                    name: std::mem::take(member),
                    args: Vec::new(),
                };
            }
            _ => {}
        }
    }
}

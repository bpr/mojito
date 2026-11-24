//! Trait defaults as upstream emits them: each default is its trait's member
//! `Trait.method`, a method of its trait over the trait's `Self`, checked
//! and lowered once, and a conformer none of whose own members has a
//! default's resolved signature gets a member over that signature whose
//! body is one method call of the trait's member on its receiver
//! (`StructEmitter::synthesizeDefaultTraitMethodWrapper` and
//! `populateDefaultedTraitFunction`). The elaborator elaborates a default
//! inside its trait; the check checks the member after every module
//! declaration is bound, and builds the wrapper.

use std::collections::{HashMap, HashSet};

use mojito_ast::ast::{
    ArgConvention, DeclaredParam, Expr, ExprKind, KwArg, Method, MethodProvenance, ParamArg,
    ParamKind, Stmt, StmtKind, TraitMethod, TypeParam, trait_entry_names,
};
use mojito_ast::call::CompileTimeParam;
use mojito_ast::visit::{MutVisitor, walk_block_mut};
use mojito_checked::checked::{CheckedDefaultWrapper, CheckedTraitMember, SignatureParam};
use mojito_common::error::TypeError;
use mojito_common::token::{Span, SyntaxId};
use mojito_symbol::symbol::OverloadLookup;
use mojito_types::types::ParamDecl;

use super::declarations::{BodyParam, MethodBody};
use super::origins::ref_binding_is_writable;
use super::overload_support::method_lowered_name;
use super::traits_support::same_resolved_shape;
use super::{Checker, MethodSig, StructDeclaration, Ty};
use mojito_symbol::symbol::lifecycle_method_name;

/// A trait's own default, as its declaration gives it: what a conformer
/// that inherits it binds and forwards, whichever program declared the
/// trait.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct DefaultShape {
    pub(super) method: String,
    /// Its position among its trait's methods: its identity, as the
    /// declaration it is.
    declared_at: usize,
    /// Its trait's member symbol, `Trait.method`, spelled from its
    /// requirement's typed signature when the name is overloaded, as its
    /// binders are owned; the trait's check records it
    /// ([`Self::at_requirement`]), so `None` until its trait registers.
    pub(super) member: Option<String>,
    /// Its requirement's position among its trait's same-name requirement
    /// signatures, which the trait's check records.
    overload: usize,
    /// Its declared parameter count, as the overload prescan counts it.
    pub(super) arity: usize,
    /// Its own compile-time parameters' names, a pack's without its `*`.
    binders: Vec<String>,
    /// Every declared parameter but the receiver, in order. Their defaults
    /// are the member's, which its declaration holds.
    pub(super) params: Vec<SignatureParam>,
    receiver: Option<ArgConvention>,
    /// Its `/` and `*` markers, as its declaration places them among
    /// [`Self::params`].
    positional_only: Option<usize>,
    keyword_only: Option<usize>,
    /// Whether it declares a result, which the forwarding call returns.
    returns: bool,
    /// Whether it declares `raises`.
    raises: bool,
    /// The compile-time parameters the forwarding call binds by name.
    forwarded: Vec<String>,
    /// Why no wrapper can forward to it, when none can.
    unforwardable: Option<String>,
    /// The member's signature as its trait's method, over the trait's
    /// `Self` (its `_Self` binder, which its receiver binds), which the
    /// trait's check records: what a wrapper's forwarding method call
    /// selects, as upstream's wrapper names its trait's function. No
    /// module binding names the member.
    pub(super) signature: Option<MethodSig>,
}

impl DefaultShape {
    /// This default, its requirement the `overload`-th signature of its
    /// name, which its trait's member `member` is named from, declaring
    /// the classified binders `declared`: the forwarding call binds each
    /// argument binder by name, as `populateDefaultedTraitFunction`
    /// forwards from the resolved signature.
    pub(super) fn at_requirement(
        self,
        overload: usize,
        member: String,
        declared: &[ParamDecl],
    ) -> Self {
        let unforwardable = self.unforwardable.or_else(|| {
            declared
                .iter()
                .any(ParamDecl::is_origin)
                .then(|| names_origin_reason(&self.method))
        });
        let forwarded = ParamDecl::argument_binders(declared)
            .filter(|decl| !decl.infer_only() && !decl.variadic() && !decl.is_pack_list())
            .map(|decl| decl.keyword().to_string())
            .collect();
        Self {
            overload,
            member: Some(member),
            forwarded,
            unforwardable,
            ..self
        }
    }

    /// Whether `other` is this default: the same declaration of the same
    /// trait.
    fn same_default(&self, trait_name: &str, other: (&str, &Self)) -> bool {
        (trait_name, self.declared_at) == (other.0, other.1.declared_at)
    }
}

/// What a trait gives a conformer: its refinements, and its own defaults.
pub(super) type TraitDefaults = (Vec<String>, Vec<DefaultShape>);

/// A default a conformer inherits: the trait declaring it, its shape, and
/// the conditional conformance it is inherited under, `None` when one
/// reaching it holds unconditionally.
pub(super) type InheritedDefault = (String, DefaultShape, Option<String>);

/// A trait default the check checked as its trait's member.
pub(super) struct TraitMember {
    module: Option<String>,
    trait_name: String,
    /// Its position among its trait's methods.
    method_index: usize,
    /// Its member symbol ([`DefaultShape::member`]).
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
    /// The conditional conformance it is inherited under, whose compiled
    /// header condition is its availability
    /// ([`Checker::conformance_availability`]).
    pub(super) gate: Option<String>,
    /// Its member position: after the struct's spelled methods, in the order
    /// its surviving defaults are reached, which registration assigns.
    pub(super) method_index: usize,
    /// The inheriting struct statement's span and syntax, which its body's
    /// syntax derives from.
    span: Span,
    syntax_id: SyntaxId,
    /// Its forwarding body, built once its trait has registered the
    /// member it calls ([`Checker::register_default_wrappers`]).
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
pub(super) fn default_shapes(methods: &[TraitMethod]) -> Vec<DefaultShape> {
    methods
        .iter()
        .enumerate()
        .filter(|(_, method)| method.default_body.is_some())
        .map(|(declared_at, method)| DefaultShape {
            method: method.name.clone(),
            declared_at,
            member: None,
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
            positional_only: method.positional_only,
            keyword_only: method.keyword_only,
            returns: method.ret.is_some(),
            raises: method.raises,
            forwarded: Vec::new(),
            unforwardable: unforwardable(method),
            signature: None,
        })
        .collect()
}

/// The defaults struct `stmt` inherits, each with the trait that declares
/// it: its conformances' refinements first, each default once however many
/// paths reach it. Each is a stub of the struct until its signatures
/// resolve, when one its own members provide drops
/// ([`Checker::register_default_wrappers`]). `traits` gives a trait's
/// refinements and its own defaults.
pub(super) fn inherited_defaults(
    stmt: &Stmt,
    traits: &dyn Fn(&str) -> Option<TraitDefaults>,
) -> Result<Vec<InheritedDefault>, TypeError> {
    let StmtKind::Struct {
        conforms,
        conformance_conditions,
        ..
    } = &stmt.kind
    else {
        return Ok(Vec::new());
    };
    // A default is as available as the conformances reaching it: always,
    // when one of them holds unconditionally.
    let mut reached: Vec<(String, DefaultShape)> = Vec::new();
    let mut conditions: HashMap<(String, usize), Option<&str>> = HashMap::new();
    // A conformance opted out with `not` (its condition `False`) brings no
    // defaults: it never holds.
    for conformance in trait_entry_names(conforms) {
        let entry = conformance_conditions
            .iter()
            .find(|(trait_name, _)| trait_name.identifier() == Some(conformance));
        if entry.is_some_and(|(_, condition)| condition.is_never_condition()) {
            continue;
        }
        let condition = entry.map(|_| conformance);
        let mut through = Vec::new();
        defaults_for(conformance, traits, &mut HashSet::new(), &mut through)?;
        for (trait_name, default) in through {
            let identity = (trait_name.clone(), default.declared_at);
            if !reached
                .iter()
                .any(|(seen_trait, seen)| default.same_default(&trait_name, (seen_trait, seen)))
            {
                reached.push((trait_name, default));
            }
            conditions
                .entry(identity)
                .and_modify(|known| *known = known.and(condition))
                .or_insert(condition);
        }
    }
    Ok(reached
        .into_iter()
        .map(|(trait_name, default)| {
            let condition = conditions
                .get(&(trait_name.clone(), default.declared_at))
                .copied()
                .flatten()
                .map(str::to_string);
            (trait_name, default, condition)
        })
        .collect())
}

/// The stub of every default struct `stmt` inherits, as
/// [`inherited_defaults`] reaches them: a wrapper once its struct's
/// signatures resolve and none of its own members provides it, its body
/// built then ([`Checker::register_default_wrappers`]).
pub(super) fn default_wrappers(
    stmt: &Stmt,
    traits: &dyn Fn(&str) -> Option<TraitDefaults>,
) -> Result<Vec<DefaultWrapper>, TypeError> {
    let StmtKind::Struct { name, methods, .. } = &stmt.kind else {
        return Ok(Vec::new());
    };
    Ok(inherited_defaults(stmt, traits)?
        .into_iter()
        .map(|(trait_name, shape, gate)| DefaultWrapper {
            module: stmt.module.clone(),
            declaration: name.clone(),
            lowered_name: format!("{name}.{}", shape.method),
            trait_name,
            shape,
            gate,
            method_index: methods.len(),
            span: stmt.span,
            syntax_id: stmt.syntax_id,
            body: Vec::new(),
        })
        .collect())
}

impl Checker {
    /// The stub of every default a top-level struct of `stmts` inherits,
    /// each counted among its struct's same-name methods for the overload
    /// prescan, as upstream's struct declares one per inherited default
    /// before its signatures resolve; one an own member provides still
    /// counts once it drops ([`Checker::register_default_wrappers`]). An
    /// own trait's defaults are read from its declaration, a package
    /// trait's from its installed record.
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
                        trait_entry_names(refines).map(str::to_string).collect(),
                        default_shapes(methods),
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

    /// Resolve every stub struct `declaration` inherits, after its spelled
    /// methods, as `signatureResolveDefaultTraitFnStubs` does: the
    /// default's resolved signature with `Self` the struct, which drops
    /// when one of the struct's own members has its signature
    /// (`filterOverloadSetForValueType`), and conflicts when an earlier
    /// stub of another trait survives with it. A surviving stub is
    /// registered as its wrapper, its binders owned by the wrapper, as
    /// `synthesizeDefaultTraitMethodWrapper` builds the wrapper's `FnOp`
    /// from `getTraitFunctionSignature`. Its body is built here, once its
    /// trait has registered the member it calls, one statement calling
    /// that member with `Self` bound first and every argument forwarded,
    /// as `populateDefaultedTraitFunction` forwards it.
    ///
    /// Each wrapper's syntax is derived from the struct statement's
    /// identity, so every check of the program builds the same occurrences.
    pub(super) fn register_default_wrappers(
        &mut self,
        declaration: &StructDeclaration<'_>,
    ) -> Result<(), TypeError> {
        let name = declaration.name;
        let self_ty = self.self_ty.clone().ok_or_else(|| {
            TypeError::InvariantViolation(format!("struct '{name}' is not in scope"))
        })?;
        // The struct's own members, which its registered wrappers follow.
        let spelled: HashMap<String, usize> = self
            .structs
            .get(name)
            .map(|info| {
                info.methods
                    .iter()
                    .map(|(method, own)| (method.clone(), own.len()))
                    .collect()
            })
            .unwrap_or_default();
        let mut ids = None;
        let mut survivors: Vec<(String, String, MethodSig)> = Vec::new();
        let mut suppressed = Vec::new();
        let mut method_index = declaration.methods.len();
        for slot in 0..self.default_wrappers.len() {
            let wrapper = &self.default_wrappers[slot];
            if !wrapper.inherited_by(declaration) {
                continue;
            }
            let mut wrapper = wrapper.clone();
            wrapper.shape = self.registered_default(&wrapper)?.clone();
            let sig = self.resolved_default(&wrapper, &self_ty)?;
            let method = wrapper.shape.method.as_str();
            let provided = self
                .structs
                .get(name)
                .and_then(|info| info.methods.get(method))
                .is_some_and(|overloads| {
                    overloads
                        .iter()
                        .take(spelled.get(method).copied().unwrap_or_default())
                        .any(|own| same_resolved_shape(own, &sig))
                });
            if provided {
                suppressed.push(slot);
                continue;
            }
            if let Some((original, ..)) = survivors
                .iter()
                .find(|(_, seen, resolved)| seen == method && same_resolved_shape(resolved, &sig))
            {
                return Err(TypeError::ConflictingTraitDefaults {
                    method: wrapper.shape.method,
                    original: original.clone(),
                    conflicting: wrapper.trait_name,
                });
            }
            if let Some(conflict) = wrapper.shape.binders.iter().find(|binder| {
                declaration
                    .type_params
                    .iter()
                    .any(|own| param_name(own) == binder.as_str())
            }) {
                return Err(TypeError::TraitDefaultParameterConflict(conflict.clone()));
            }
            if let Some(reason) = &wrapper.shape.unforwardable {
                return Err(TypeError::Unsupported(reason.clone()));
            }
            survivors.push((wrapper.trait_name.clone(), method.to_string(), sig.clone()));
            wrapper.method_index = method_index;
            method_index += 1;
            let mut body = vec![forwarding_statement(&wrapper.shape, wrapper.span)?];
            if let Some(module) = &wrapper.module {
                mojito_ast::ast::stamp_source(&mut body, module);
            }
            walk_block_mut(
                ids.get_or_insert(DeriveSyntax {
                    parent: wrapper.syntax_id,
                    next: 0,
                }),
                &mut body,
            );
            wrapper.body = body;
            let signature;
            (wrapper.lowered_name, signature) =
                self.register_default_wrapper(declaration, &wrapper, sig)?;
            self.list_default_wrapper(
                declaration,
                &wrapper.shape.method,
                wrapper.method_index,
                &wrapper.lowered_name,
                &wrapper.shape.params,
                &signature,
            );
            self.default_wrappers[slot] = wrapper;
        }
        let provided: Vec<(String, usize)> = suppressed
            .iter()
            .map(|slot| {
                let shape = &self.default_wrappers[*slot].shape;
                (shape.method.clone(), shape.arity)
            })
            .collect();
        for slot in suppressed.into_iter().rev() {
            self.default_wrappers.remove(slot);
        }
        if let Some(info) = self.structs.get_mut(name) {
            info.provided_defaults = provided;
        }
        Ok(())
    }

    /// Check every default a top-level trait of `stmts` declares as its
    /// trait's member, once every module declaration is bound, as upstream
    /// emits a default inside its trait: its body names any function of the
    /// module, wherever declared. The member's signature is recorded on its
    /// trait's default, through which only a conformer's wrapper calls it
    /// ([`Checker::trait_member_signature`]).
    pub(super) fn check_trait_members(&mut self, stmts: &[Stmt]) -> Result<(), TypeError> {
        for stmt in stmts {
            let StmtKind::Trait { name, methods, .. } = &stmt.kind else {
                continue;
            };
            for (index, method) in methods.iter().enumerate() {
                let Some(member) = member_method(method) else {
                    if method.params.iter().any(|param| param.default.is_some()) {
                        self.check_requirement_defaults(
                            stmt.module.as_ref(),
                            (name, index),
                            method,
                        )?;
                    }
                    continue;
                };
                let symbol = self
                    .traits
                    .get(name)
                    .and_then(|info| {
                        info.defaults
                            .iter()
                            .find(|default| default.declared_at == index)
                    })
                    .and_then(|default| default.member.clone())
                    .ok_or_else(|| {
                        TypeError::InvariantViolation(format!(
                            "trait '{name}' records no member for default '{}'",
                            method.name
                        ))
                    })?;
                self.check_trait_member(stmt.module.as_ref(), (name, index), &member, &symbol)?;
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

    /// The trait and signature of trait member `symbol`, `Trait.method`,
    /// from its trait's record, own or installed: what the forwarding method call of
    /// a conformer's wrapper selects. Source cannot spell a dotted member,
    /// so no other call reaches it.
    pub(super) fn trait_member_signature<'s>(
        &self,
        symbol: &'s str,
    ) -> Option<(&'s str, &MethodSig)> {
        let (trait_name, _) = symbol.split_once('.')?;
        self.traits
            .get(trait_name)?
            .defaults
            .iter()
            .find(|default| default.member.as_deref() == Some(symbol))?
            .signature
            .as_ref()
            .map(|signature| (trait_name, signature))
    }

    /// Check the body of every wrapper a top-level struct of `stmts`
    /// inherits, after every trait member its wrappers call is bound, each
    /// the next of its name's overloads after the struct's spelled methods.
    pub(super) fn check_default_wrappers(&mut self, stmts: &[Stmt]) -> Result<(), TypeError> {
        for stmt in stmts {
            let Some(declaration) = super::overload_support::struct_declaration(stmt) else {
                continue;
            };
            let wrappers: Vec<(usize, DefaultWrapper)> = self
                .default_wrappers
                .iter()
                .enumerate()
                .filter(|(_, wrapper)| wrapper.inherited_by(&declaration))
                .map(|(position, wrapper)| (position, wrapper.clone()))
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
            let checked = wrappers.iter().try_for_each(|(position, wrapper)| {
                let overload_index = overload_indices
                    .entry(wrapper.shape.method.clone())
                    .or_default();
                let index = *overload_index;
                *overload_index += 1;
                self.emitter
                    .get_mut()
                    .enter_wrapper_body(*position, &wrapper.body);
                self.check_default_wrapper(&self_ty, wrapper, index)
            });
            self.exit_struct_scope(saved);
            checked?;
        }
        Ok(())
    }

    /// The checked handoff of every trait member.
    pub(super) fn checked_trait_members(&self) -> Vec<CheckedTraitMember> {
        self.trait_members
            .iter()
            .map(|member| CheckedTraitMember {
                module: member.module.clone(),
                trait_name: member.trait_name.clone(),
                method_index: member.method_index,
                symbol: member.lowered_name.clone(),
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
                member: wrapper.shape.member.clone().unwrap_or_default(),
                self_convention: wrapper.shape.receiver,
                params: wrapper.shape.params.clone(),
                positional_only: wrapper.shape.positional_only,
                keyword_only: wrapper.shape.keyword_only,
                signature: mojito_checked::checked::CheckedSignature::default(),
            })
            .collect()
    }

    /// `wrapper`'s default's signature with `Self` the conformer `self_ty`.
    fn resolved_default(
        &self,
        wrapper: &DefaultWrapper,
        self_ty: &Ty,
    ) -> Result<MethodSig, TypeError> {
        let method = wrapper.shape.method.as_str();
        let requirement = self
            .traits
            .get(&wrapper.trait_name)
            .and_then(|info| {
                info.methods
                    .get(&wrapper.shape.method)?
                    .get(wrapper.shape.overload)
            })
            .cloned()
            .ok_or_else(|| {
                TypeError::InvariantViolation(format!(
                    "trait '{}' records no default '{method}'",
                    wrapper.trait_name
                ))
            })?;
        Ok(self.requirement_at(&requirement, self_ty))
    }

    /// Register `wrapper`'s signature `sig`, its default's resolved at the
    /// conformer, on struct `declaration` as the wrapper's own, returning
    /// the symbol it lowers to and the signature.
    fn register_default_wrapper(
        &mut self,
        declaration: &StructDeclaration<'_>,
        wrapper: &DefaultWrapper,
        mut sig: MethodSig,
    ) -> Result<(String, MethodSig), TypeError> {
        use mojito_checked::checked::{AnnotationSite, DeclarationEffect, GenericSite};
        let name = declaration.name;
        let method = wrapper.shape.method.as_str();
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
            &sig.declared,
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
        sig.declared = decls;
        sig.params = sig.params.iter().map(moved).collect();
        for ty in [&mut sig.variadic, &mut sig.kw_variadic, &mut sig.error]
            .into_iter()
            .flatten()
        {
            *ty = moved(ty);
        }
        sig.ret = moved(&sig.ret);
        if let Some(gate) = &wrapper.gate {
            sig.availability
                .extend(self.conformance_availability(name, gate)?);
        }
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
            sig.declared.clone(),
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
        if wrapper.shape.receiver == Some(ArgConvention::Deinit) && method != "__deinit__" {
            info.explicit_destructors
                .entry(method.to_string())
                .or_insert(sig.raises);
        }
        info.methods
            .entry(method.to_string())
            .or_default()
            .push(sig.clone());
        Ok((lowered, sig))
    }

    /// Check default `m` of trait `trait_name`, its `index`-th method, as
    /// that trait's method named `symbol`: its signature registered at its
    /// trait's method sites, its receiver `self: _Self`, and its body
    /// checked in the trait's scope, where `Self` is the trait's `_Self`
    /// binder ([`TraitDeclaration::self_binder`]).
    fn check_trait_member(
        &mut self,
        module: Option<&String>,
        (trait_name, index): (&str, usize),
        m: &Method,
        symbol: &str,
    ) -> Result<(), TypeError> {
        let owner = super::annotations::binder_owner(symbol);
        let (self_ty, saved) = self.enter_trait_scope(trait_name);
        let checked = self
            .register_trait_member(module, (trait_name, index), m, (&owner, symbol))
            .and_then(|availability| {
                self.check_member(
                    &self_ty,
                    m,
                    (module, trait_name, index),
                    &owner,
                    &availability,
                    symbol.to_string(),
                )
            });
        self.exit_struct_scope(saved);
        checked
    }

    /// Type default `m` of trait `trait_name`, its `index`-th method, in its
    /// trait's scope, its own binders owned by `owner` after its trait's
    /// `_Self`; record it at its trait's method sites and on its trait's
    /// default as the member `symbol`, and return its availability.
    fn register_trait_member(
        &mut self,
        module: Option<&String>,
        (trait_name, index): (&str, usize),
        m: &Method,
        (owner, symbol): (&str, &str),
    ) -> Result<Vec<super::GenericConstraint>, TypeError> {
        let method_decls = self.classify_param_list_after(
            owner,
            &m.type_params,
            self.enclosing_type_params.len(),
        )?;
        self.push_param_scope(&method_decls);
        let saved_type_params = self.enclosing_type_params.clone();
        let saved_decls = self.enclosing_param_decls.clone();
        let saved_struct_count = self
            .enclosing_struct_type_params
            .replace(saved_type_params.len());
        self.enclosing_type_params.extend(m.type_params.clone());
        self.enclosing_param_decls
            .extend(method_decls.iter().cloned());
        let signature = self.compiled_method_signature(trait_name, m, method_decls);
        self.enclosing_type_params = saved_type_params;
        self.enclosing_param_decls = saved_decls;
        self.enclosing_struct_type_params.set(saved_struct_count);
        self.pop_param_scope();
        let (all_types, mut sig) = signature?;
        sig.overload = mojito_symbol::symbol::overload_qualifier(symbol).map(str::to_string);
        self.record_method_sites((module, trait_name, index), &all_types, &sig);
        if symbol != format!("{trait_name}.{}", m.name) {
            self.declaration_symbols.borrow_mut().insert(
                mojito_checked::checked::GenericSite::Method {
                    module: module.cloned(),
                    declaration: trait_name.to_string(),
                    method: index,
                },
                symbol.to_string(),
            );
        }
        let availability = sig.availability.clone();
        if let Some(default) = self.traits.get_mut(trait_name).and_then(|info| {
            info.defaults
                .iter_mut()
                .find(|default| default.declared_at == index)
        }) {
            default.signature = Some(sig);
        }
        Ok(availability)
    }

    /// `wrapper`'s default as its trait registered it, its member named.
    fn registered_default(&self, wrapper: &DefaultWrapper) -> Result<&DefaultShape, TypeError> {
        self.traits
            .get(&wrapper.trait_name)
            .and_then(|info| {
                info.defaults
                    .iter()
                    .find(|default| default.declared_at == wrapper.shape.declared_at)
            })
            .ok_or_else(|| {
                TypeError::InvariantViolation(format!(
                    "trait '{}' records no default '{}'",
                    wrapper.trait_name, wrapper.shape.method
                ))
            })
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
        let saved_site = self
            .method_site
            .replace((wrapper.module.clone(), declaration.to_string()));
        self.push_param_scope(&signature.decls());
        let saved_struct_count = self
            .enclosing_struct_type_params
            .replace(self.enclosing_type_params.len());
        let mut assumptions = Vec::new();
        for gate in &signature.availability {
            self.guaranteed_facts(gate, &mut assumptions);
        }
        assumptions.extend(self.method_propositions(&signature.availability, &signature.decls()));
        self.push_assumptions(assumptions);
        let result = self.check_default_wrapper_body(self_ty, wrapper, &signature, effect_key);
        self.pop_assumptions();
        self.enclosing_struct_type_params.set(saved_struct_count);
        self.pop_param_scope();
        self.method_site = saved_site;
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
            self.declare_value_parameters(&signature.decls())?;
            let self_writable =
                ref_binding_is_writable(signature.self_convention, None, &[], &*self);
            self.bind_receiver(self_ty, self_writable)?;
            let mut regular = signature.params.iter();
            for param in &wrapper.shape.params {
                let pty = match param.kind {
                    ParamKind::Regular => regular.next().cloned(),
                    ParamKind::Variadic => signature.variadic.clone().map(|element| {
                        mojito_types::types::variadic_collector_type(element, param.convention)
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
                    ref_binding_is_writable(param.convention, None, &[], &*self),
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
                method_decls: &signature.decls(),
                ref_return: signature.ref_return.clone(),
                return_annotation: None,
                generic: !signature.decls().is_empty(),
                ret_ty: &signature.ret,
                body: &wrapper.body,
            })
        })();
        self.raising_context.pop();
        self.pop_scope();
        result
    }
}

/// A compile-time parameter's name without a pack's leading `*`.
fn param_name(param: &TypeParam) -> &str {
    param.name.trim_start_matches('*')
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
            .any(|(seen_trait, seen)| default.same_default(name, (seen_trait, seen)))
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
        param.kind == ParamKind::Variadic && !matches!(&param.ty.kind, ExprKind::Spread(_))
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
        || method.ret_reference.is_some();
    names_origin.then(|| names_origin_reason(name))
}

/// Why no conformer's wrapper can forward to default `name`, whose
/// signature names an origin.
fn names_origin_reason(name: &str) -> String {
    format!(
        "inheriting trait default '{name}', whose signature names an origin: a \
         conformer's wrapper over such a signature is not supported yet"
    )
}

/// The one statement of a wrapper for `shape`: a method call of its trait's
/// member on the receiver, which binds the member's `_Self`, every argument
/// forwarded with the convention it declares and every forwarded binder
/// bound by name.
fn forwarding_statement(shape: &DefaultShape, span: Span) -> Result<Stmt, TypeError> {
    let expr = |kind| Expr::new(kind, span);
    let name = |name: &str| expr(ExprKind::Identifier(name.to_string()));
    let transfer = |param: &str| expr(ExprKind::Transfer(Box::new(name(param))));
    let owned = |convention: Option<ArgConvention>| {
        matches!(convention, Some(ArgConvention::Var | ArgConvention::Deinit))
    };
    let param_args: Vec<ParamArg> = shape
        .forwarded
        .iter()
        .map(|param| ParamArg::Named {
            name: param.clone(),
            value: Box::new(ParamArg::Value(name(param))),
        })
        .collect();
    let receiver = Box::new(if owned(shape.receiver) {
        transfer("self")
    } else {
        name("self")
    });
    let mut args = Vec::new();
    let mut kwargs = Vec::new();
    let mut keyword = false;
    for (index, param) in shape.params.iter().enumerate() {
        keyword |= shape.keyword_only == Some(index);
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
    let member = shape.member.clone().ok_or_else(|| {
        TypeError::InvariantViolation(format!(
            "the trait declaring default '{}' has not registered",
            shape.method
        ))
    })?;
    let call = expr(if param_args.is_empty() {
        ExprKind::MethodCall {
            object: receiver,
            method: member,
            args,
            kwargs,
        }
    } else {
        ExprKind::Invoke {
            callee: Box::new(expr(ExprKind::Member {
                object: receiver,
                field: member,
            })),
            param_args,
            args,
            kwargs,
        }
    });
    let statement = if shape.returns {
        StmtKind::Return(Some(call))
    } else {
        StmtKind::Expr(call)
    };
    Ok(Stmt::new(statement, span))
}

/// Trait default `method` as its trait's method, its body the default's,
/// whose `Self` the trait's scope resolves to the trait's `_Self` binder.
/// `None` when the method declares no default.
fn member_method(method: &TraitMethod) -> Option<Method> {
    let body = method.default_body.clone()?;
    Some(Method {
        name: method.name.clone(),
        type_params: method.type_params.clone(),
        has_self: true,
        self_convention: method.self_convention,
        self_origin: method.self_origin.clone(),
        self_type: None,
        receiver_index: 0,
        decorators: method.decorators.clone(),
        params: method.params.clone(),
        positional_only: method.positional_only,
        keyword_only: method.keyword_only,
        raises: method.raises,
        raises_type: method.raises_type.clone(),
        ret: method.ret.clone(),
        ret_reference: method.ret_reference.clone(),
        where_clauses: method.where_clauses.clone(),
        body,
        provenance: MethodProvenance::Source,
    })
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

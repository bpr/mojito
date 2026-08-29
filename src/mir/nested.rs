//! Recursive nested-function capture discovery and MIR lifting.

use super::*;

// --- Nested `def` (closure) lifting -----------------------------------------

/// Lower a function body (`name` its registered/mangled name) plus every nested
/// `def` it defines, pushing the function and each lifted nested function into
/// `out`. A liftable nested `def` becomes `name$inner` with its captured enclosing
/// locals as leading `mut` parameters (checker-typed where declared and opaque
/// for captured storage);
/// a nested `def` we can't lift stays a clean `Unsupported` at execution.
pub(super) struct FunctionLowering<'a> {
    pub(super) checked: &'a crate::CheckedProgram,
    pub(super) name: &'a str,
    pub(super) parameter_names: &'a [String],
    pub(super) parameter_types: Vec<Ty>,
    /// Hidden frame locals populated from compile-time value-argument registers.
    /// They are predeclared for name/type resolution but do not increase the
    /// function's ordinary runtime parameter count.
    pub(super) value_parameter_locals: Vec<(String, Ty)>,
    pub(super) owned_parameters: Vec<bool>,
    pub(super) deinit_parameters: Vec<bool>,
    pub(super) reference_parameters: Vec<bool>,
    pub(super) returns_reference: bool,
    /// Checked return type and raising contract of the declaration.
    pub(super) ret_ty: Ty,
    pub(super) raises: bool,
    pub(super) error_ty: Option<Ty>,
    pub(super) named_result: Option<&'a str>,
    pub(super) body: &'a [Stmt],
    pub(super) overloads: &'a crate::symbol::OverloadSets,
}

pub(super) fn lower_fn_nested(
    request: FunctionLowering<'_>,
    out: &mut Vec<(String, MirFunction)>,
    declarations: &mut MirDeclarations,
) {
    let FunctionLowering {
        checked,
        name,
        parameter_names: param_names,
        parameter_types: param_types,
        value_parameter_locals,
        owned_parameters: owned_params,
        deinit_parameters: deinit_params,
        reference_parameters: ref_params,
        returns_reference,
        ret_ty,
        raises,
        error_ty,
        named_result,
        body,
        overloads,
    } = request;
    let mut children = analyze_root_children(checked, name, param_names, body);
    let registry = scope_registry(body, &mut children, &HashMap::new());

    let mut cfg = Cfg::build_checked_fn(checked, param_names, body);
    // Parameter slot types come from the checked declaration, not from
    // name-matched body expressions (same-named bindings in other functions
    // must never leak into this function's slots).
    for (slot, ty) in param_types.iter().enumerate() {
        cfg.var_types.insert(slot as VarId, ty.clone());
    }
    for (name, ty) in &value_parameter_locals {
        let slot = cfg
            .vars
            .iter()
            .position(|candidate| candidate == name)
            .expect("value parameter was seeded into the function CFG");
        cfg.var_types.insert(slot as VarId, ty.clone());
    }
    let mut f = lower_cfg_nested(
        &cfg,
        &registry,
        overloads,
        returns_reference,
        &ref_params,
        &[],
        checked.call_transfers(),
    );
    f.n_params = param_types.len();
    for (slot, ty) in param_types.iter().enumerate() {
        f.var_tys.entry(slot as VarId).or_insert_with(|| ty.clone());
    }
    for (name, ty) in &value_parameter_locals {
        let slot = f
            .var_names
            .iter()
            .position(|candidate| candidate == name)
            .expect("value parameter was retained in the MIR frame");
        f.var_tys.entry(slot as VarId).or_insert_with(|| ty.clone());
    }
    f.param_types = param_types;
    f.owned_params = owned_params;
    f.deinit_params = deinit_params;
    f.ref_params = ref_params;
    f.returns_reference = returns_reference;
    f.ret_ty = Some(ret_ty);
    f.raises = raises;
    f.error_ty = error_ty;
    if let Some(result) = named_result {
        materialize_named_result_return(&mut f, result);
    }
    out.push((name.to_string(), f));

    for child in children {
        lower_nested_node(checked, child, &registry, overloads, out, declarations);
    }
}

/// Collect the declarations belonging to one lexical function scope (those in
/// the body or its control-flow blocks), without descending into another
/// declaration's body. Recursive lifting calls this once per lexical scope.
fn find_nested_defs<'a>(body: &'a [Stmt], out: &mut Vec<&'a Stmt>) {
    for s in body {
        // Lambda expressions anywhere in this statement's own expressions
        // (conditions, iterables, values, arguments) declare hidden nested
        // definitions in this scope.
        let mut lambdas = Vec::new();
        crate::ast::lambdas_in_stmt(s, &mut lambdas);
        for lambda in lambdas {
            if let crate::ast::ExprKind::Lambda { def } = &lambda.kind {
                out.push(def);
            }
        }
        match &s.kind {
            StmtKind::Def { .. } => out.push(s),
            StmtKind::If { branches, orelse } => {
                for (_, b) in branches {
                    find_nested_defs(b, out);
                }
                if let Some(e) = orelse {
                    find_nested_defs(e, out);
                }
            }
            StmtKind::While { body, orelse, .. } | StmtKind::For { body, orelse, .. } => {
                find_nested_defs(body, out);
                if let Some(orelse) = orelse {
                    find_nested_defs(orelse, out);
                }
            }
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                find_nested_defs(body, out);
                if let Some((_, b)) = except {
                    find_nested_defs(b, out);
                }
                if let Some(e) = orelse {
                    find_nested_defs(e, out);
                }
                if let Some(f) = finalbody {
                    find_nested_defs(f, out);
                }
            }
            _ => {}
        }
    }
}

/// One declaration in the lexical closure tree. Checked capture facts already
/// include any ancestor forwarding required across intermediate functions.
struct NestedNode<'a> {
    statement: &'a Stmt,
    binding: crate::origin::OwnerId,
    source_name: String,
    mangled: String,
    captures: Vec<NestedCapture>,
    callable_ty: Option<Ty>,
    children: Vec<NestedNode<'a>>,
}

fn analyze_node<'a>(
    checked: &crate::CheckedProgram,
    statement: &'a Stmt,
    parent_mangled: &str,
    duplicate_name: bool,
) -> NestedNode<'a> {
    let StmtKind::Def { name, body, .. } = &statement.kind else {
        unreachable!("nested tree contains only def statements")
    };

    let declaration = checked_nested_declaration(checked, statement);
    let binding = declaration
        .binding
        .expect("checked nested function has no value binding");

    let mangled = if duplicate_name {
        crate::symbol::nested_lifted_declaration_name(parent_mangled, name, declaration.id)
    } else {
        crate::symbol::nested_lifted_name(parent_mangled, name)
    };
    let captures = analyze_captures(declaration);

    let mut direct = Vec::new();
    find_nested_defs(body, &mut direct);
    let mut totals = HashMap::<String, usize>::new();
    for declaration in &direct {
        if let StmtKind::Def { name, .. } = &declaration.kind {
            *totals.entry(name.clone()).or_default() += 1;
        }
    }
    let children = direct
        .into_iter()
        .filter_map(|nested| {
            let StmtKind::Def { name, .. } = &nested.kind else {
                return None;
            };
            Some(analyze_node(
                checked,
                nested,
                &mangled,
                totals.get(name).copied().unwrap_or_default() > 1,
            ))
        })
        .collect();

    NestedNode {
        statement,
        binding,
        source_name: name.clone(),
        mangled,
        captures,
        callable_ty: checked
            .checked_type_at(&AnnotationSite::FunctionType {
                module: statement.module.clone(),
                declaration: statement.span,
                syntax: statement.syntax_id,
            })
            .cloned()
            .map(mir_nested_callable_ty),
        children,
    }
}

/// Build the callable registry visible in one lexical function. Direct
/// declarations and inherited callables are keyed by checked identity, so
/// same-spelled bindings coexist without shadow-removal heuristics.
fn scope_registry(
    _body: &[Stmt],
    children: &mut [NestedNode<'_>],
    inherited: &HashMap<crate::origin::OwnerId, NestedInfo>,
) -> HashMap<crate::origin::OwnerId, NestedInfo> {
    let mut registry = inherited.clone();
    for info in registry.values_mut() {
        info.materialized_here = false;
    }
    for child in children.iter() {
        registry.insert(
            child.binding,
            NestedInfo {
                binding: child.binding,
                source_name: child.source_name.clone(),
                mangled: child.mangled.clone(),
                materialized_here: true,
                captures: child.captures.clone(),
                callable_ty: child.callable_ty.clone(),
            },
        );
    }

    registry
}

fn lower_nested_node(
    checked: &crate::CheckedProgram,
    mut node: NestedNode<'_>,
    inherited_registry: &HashMap<crate::origin::OwnerId, NestedInfo>,
    overloads: &crate::symbol::OverloadSets,
    out: &mut Vec<(String, MirFunction)>,
    declarations: &mut MirDeclarations,
) {
    let ds = node.statement;
    if let StmtKind::Def {
        params: dparams,
        positional_only,
        keyword_only,
        body: dbody,
        ..
    } = &ds.kind
    {
        let registry = scope_registry(dbody, &mut node.children, inherited_registry);
        let captures = &node.captures;
        let mangled = &node.mangled;
        let capture_types: Vec<Ty> = captures
            .iter()
            .map(|capture| mir_nested_callable_ty(capture.ty.clone()))
            .collect();
        let named_result = dparams
            .iter()
            .find(|parameter| matches!(parameter.convention, Some(ArgConvention::Out)));
        let caller_params: Vec<_> = dparams
            .iter()
            .enumerate()
            .filter(|(_, parameter)| !matches!(parameter.convention, Some(ArgConvention::Out)))
            .collect();
        let mut names: Vec<String> = captures
            .iter()
            .map(|capture| capture.name.clone())
            .collect();
        names.extend(
            caller_params
                .iter()
                .map(|(_, parameter)| parameter.name.clone()),
        );
        if let Some(result) = named_result {
            names.push(result.name.clone());
        }
        let generic_site = GenericSite::Function {
            module: ds.module.clone(),
            declaration: ds.span,
            syntax: ds.syntax_id,
        };
        let param_decls = checked
            .generic_parameters_at(&generic_site)
            .unwrap_or(&[])
            .to_vec();
        let value_parameter_locals = value_parameter_locals(&param_decls);
        names.extend(value_parameter_locals.iter().map(|(name, _)| name.clone()));
        let mut ptys = capture_types.clone();
        ptys.extend(caller_params.iter().map(|(param, p)| {
            let ty = checked
                .checked_type_at(&AnnotationSite::FunctionParam {
                    module: ds.module.clone(),
                    declaration: ds.span,
                    syntax: ds.syntax_id,
                    param: *param,
                })
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "checked type missing for parameter '{}' of nested function",
                        p.name
                    )
                });
            body_parameter_ty(p, ty)
        }));
        // The closure object owns declaration-time copy/move captures. A lifted
        // invocation only borrows those environment slots, just as it borrows
        // explicit read/mut/ref captures; it must neither destroy nor duplicate
        // them on each call.
        let mut owned2: Vec<bool> = vec![false; captures.len()];
        owned2.extend(
            caller_params
                .iter()
                .map(|(_, parameter)| is_owned(&parameter.convention)),
        );
        // Captures are borrowed, never `deinit` sources.
        let mut deinit2: Vec<bool> = vec![false; captures.len()];
        deinit2.extend(
            caller_params
                .iter()
                .map(|(_, parameter)| is_deinit(&parameter.convention)),
        );
        // Captures are `mut` (their final value is written back to the enclosing
        // variable — reference-capture semantics).
        let mut refp2: Vec<bool> = vec![true; captures.len()];
        refp2.extend(
            caller_params
                .iter()
                .map(|(_, parameter)| is_ref(&parameter.convention)),
        );
        let variadic_idx = dparams.iter().position(|p| p.kind == ParamKind::Variadic);
        let kw_variadic_idx = dparams.iter().position(|p| p.kind == ParamKind::KwVariadic);
        let regular: Vec<_> = dparams
            .iter()
            .enumerate()
            .filter(|(_, parameter)| {
                parameter.kind == ParamKind::Regular
                    && !matches!(parameter.convention, Some(ArgConvention::Out))
            })
            .collect();
        let mut declaration_names: Vec<String> = captures
            .iter()
            .map(|capture| capture.name.clone())
            .collect();
        declaration_names.extend(regular.iter().map(|(_, parameter)| parameter.name.clone()));
        let mut declaration_types = capture_types;
        declaration_types.extend(regular.iter().map(|(index, parameter)| {
            checked
                .checked_type_at(&AnnotationSite::FunctionParam {
                    module: ds.module.clone(),
                    declaration: ds.span,
                    syntax: ds.syntax_id,
                    param: *index,
                })
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "checked type missing for parameter '{}' of nested function",
                        parameter.name
                    )
                })
        }));
        let return_site = AnnotationSite::FunctionReturn {
            module: ds.module.clone(),
            declaration: ds.span,
            syntax: ds.syntax_id,
        };
        let return_ty = checked
            .checked_type_at(&return_site)
            .cloned()
            .unwrap_or(Ty::None);
        let effect = checked
            .declaration_effect_at(&return_site)
            .cloned()
            .unwrap_or(crate::checked::DeclarationEffect {
                raises: false,
                error: None,
                returns_reference: false,
            });
        let capture_count = captures.len();
        declarations.functions.push(MirFunctionDeclaration {
            lowered_name: mangled.clone(),
            param_names: declaration_names,
            param_types: declaration_types,
            defaults: std::iter::repeat_n(None, capture_count)
                .chain(
                    regular
                        .iter()
                        .map(|(_, parameter)| mir_default(checked, parameter.default.as_ref())),
                )
                .collect(),
            required: std::iter::repeat_n(true, capture_count)
                .chain(
                    regular
                        .iter()
                        .map(|(_, parameter)| parameter.default.is_none()),
                )
                .collect(),
            variadic: variadic_idx.map(|index| {
                checked
                    .checked_type_at(&AnnotationSite::FunctionParam {
                        module: ds.module.clone(),
                        declaration: ds.span,
                        syntax: ds.syntax_id,
                        param: index,
                    })
                    .cloned()
                    .unwrap_or(Ty::None)
            }),
            variadic_convention: variadic_idx.and_then(|index| dparams[index].convention),
            variadic_index: runtime_variadic_index(dparams, variadic_idx)
                .map(|index| capture_count + index),
            kw_variadic: kw_variadic_idx.map(|index| {
                checked
                    .checked_type_at(&AnnotationSite::FunctionParam {
                        module: ds.module.clone(),
                        declaration: ds.span,
                        syntax: ds.syntax_id,
                        param: index,
                    })
                    .cloned()
                    .unwrap_or(Ty::None)
            }),
            kw_variadic_convention: kw_variadic_idx.and_then(|index| dparams[index].convention),
            kw_variadic_index: runtime_parameter_index(dparams, kw_variadic_idx)
                .map(|index| capture_count + index),
            positional_only: regular_marker_index(dparams, *positional_only)
                .map(|index| capture_count + index),
            keyword_only: effective_keyword_only_index(dparams, *keyword_only, variadic_idx)
                .map(|index| capture_count + index),
            param_decls,
            has_receiver: false,
            receiver_convention: None,
            param_conventions: std::iter::repeat_n(Some(ArgConvention::Ref), capture_count)
                .chain(regular.iter().map(|(_, parameter)| parameter.convention))
                .collect(),
            ret_ty: return_ty,
            returns_reference: effect.returns_reference,
            raises: effect.raises,
            error_ty: effect.error,
            ref_params: std::iter::repeat_n(true, captures.len())
                .chain(
                    regular
                        .iter()
                        .map(|(_, parameter)| is_ref(&parameter.convention)),
                )
                .collect(),
        });
        let immutable_captures: HashSet<String> = captures
            .iter()
            .filter(|capture| capture.kind == crate::ast::CaptureKind::Imm)
            .map(|capture| capture.name.clone())
            .collect();
        let mut ncfg =
            Cfg::build_checked_fn_with_captures(checked, &names, immutable_captures, dbody);
        // Every lifted slot takes its checker-owned storage type. In particular,
        // an intermediate closure may only forward a captured sibling closure
        // to a descendant, so body expression inference has no use from which
        // it could rediscover that callable type.
        for (slot, ty) in ptys.iter().enumerate() {
            ncfg.var_types.insert(slot as VarId, ty.clone());
        }
        for (name, ty) in &value_parameter_locals {
            let slot = ncfg
                .vars
                .iter()
                .position(|candidate| candidate == name)
                .expect("nested value parameter was seeded into the function CFG");
            ncfg.var_types.insert(slot as VarId, ty.clone());
        }
        let mut nf = lower_cfg_nested(
            &ncfg,
            &registry,
            overloads,
            effect.returns_reference,
            &refp2,
            &captures
                .iter()
                .map(|capture| capture.binding)
                .collect::<Vec<_>>(),
            checked.call_transfers(),
        );
        nf.n_params = ptys.len();
        for (slot, ty) in ptys.iter().enumerate() {
            nf.var_tys
                .entry(slot as VarId)
                .or_insert_with(|| ty.clone());
        }
        for (name, ty) in &value_parameter_locals {
            let slot = nf
                .var_names
                .iter()
                .position(|candidate| candidate == name)
                .expect("nested value parameter was retained in the MIR frame");
            nf.var_tys
                .entry(slot as VarId)
                .or_insert_with(|| ty.clone());
        }
        nf.param_types = ptys;
        nf.owned_params = owned2;
        nf.deinit_params = deinit2;
        nf.ref_params = refp2;
        nf.returns_reference = effect.returns_reference;
        // The nested `def` was checked as an ordinary declaration, so its
        // return and raising contract are recorded checked facts.
        nf.ret_ty = checked.checked_type_at(&return_site).cloned();
        if let Some(effect) = checked.declaration_effect_at(&return_site) {
            nf.raises = effect.raises;
            nf.error_ty = effect.error.clone();
        }
        if let Some(result) = named_result {
            materialize_named_result_return(&mut nf, &result.name);
        }
        out.push((mangled.clone(), nf));
        for child in node.children {
            lower_nested_node(checked, child, &registry, overloads, out, declarations);
        }
    }
}

fn materialize_named_result_return(function: &mut MirFunction, result: &str) {
    let slot = function
        .var_names
        .iter()
        .position(|name| name == result)
        .expect("named result was seeded into the function variables");
    for block in &mut function.blocks {
        if matches!(
            block.term,
            MirTerm::Return(None)
                | MirTerm::ReturnWithCleanup { value: None, .. }
                | MirTerm::FallOff
        ) {
            let register = Reg(function.n_regs);
            function.n_regs += 1;
            block.instrs.push(MirInstr::UseVar {
                dest: register,
                var: slot as VarId,
                mode: UseMode::Copy,
            });
            match &mut block.term {
                MirTerm::ReturnWithCleanup { value, .. } => *value = Some(register),
                _ => block.term = MirTerm::Return(Some(register)),
            }
        }
    }
}

/// `RuntimePack` distinguishes per-position variadic checking but is not a
/// runtime value type. A nested direct call is lowered through a closure-value
/// register, so erase that declaration-only discriminator to the Tuple storage
/// actually carried by the lifted function's single collector slot.
fn mir_nested_callable_ty(mut ty: Ty) -> Ty {
    match &mut ty {
        Ty::Func { variadic, .. } | Ty::GenericFunc { variadic, .. } => {
            if let Some(element) = variadic
                && let Ty::RuntimePack(elements) = &mut **element
            {
                **element = Ty::Tuple(std::mem::take(elements));
            }
        }
        _ => {}
    }
    ty
}

/// Copy the checker-resolved capture environment. This includes unused explicit
/// entries and any ancestor forwarding selected by enclosing capture policies.
fn analyze_captures(declaration: &crate::CheckedDeclaration) -> Vec<NestedCapture> {
    declaration
        .captures
        .iter()
        .map(|capture| NestedCapture {
            name: capture.name.clone(),
            binding: capture.binding,
            ty: capture.ty.clone(),
            kind: capture.kind,
        })
        .collect()
}

fn checked_nested_declaration<'a>(
    checked: &'a crate::CheckedProgram,
    statement: &Stmt,
) -> &'a crate::CheckedDeclaration {
    let StmtKind::Def { name, .. } = &statement.kind else {
        unreachable!("nested declaration lookup requires a def")
    };
    checked
        .declarations()
        .iter()
        .find(|declaration| {
            declaration.kind == crate::checked::CheckedDeclKind::Function
                && declaration.name == *name
                && declaration.location == statement.source_span()
        })
        .expect("checked nested function declaration is missing")
}

fn analyze_root_children<'a>(
    checked: &crate::CheckedProgram,
    parent_mangled: &str,
    _parameter_names: &[String],
    body: &'a [Stmt],
) -> Vec<NestedNode<'a>> {
    let mut direct = Vec::new();
    find_nested_defs(body, &mut direct);
    let mut totals = HashMap::<String, usize>::new();
    for declaration in &direct {
        if let StmtKind::Def { name, .. } = &declaration.kind {
            *totals.entry(name.clone()).or_default() += 1;
        }
    }
    direct
        .into_iter()
        .filter_map(|declaration| {
            let StmtKind::Def { name, .. } = &declaration.kind else {
                return None;
            };
            let child = analyze_node(
                checked,
                declaration,
                parent_mangled,
                totals.get(name).copied().unwrap_or_default() > 1,
            );
            Some(child)
        })
        .collect()
}

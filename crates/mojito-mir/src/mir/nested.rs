//! Recursive nested-function capture discovery and MIR lifting.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

// --- Nested `def` (closure) lifting -----------------------------------------

/// Lower a function body (`name` its registered/mangled name) plus every nested
/// `def` it defines, pushing the function and each lifted nested function into
/// `out`. A liftable nested `def` becomes `name$inner` with its captured enclosing
/// locals as leading `mut` parameters (checker-typed where declared and opaque
/// for captured storage);
/// a nested `def` we can't lift stays a clean `Unsupported` at execution.
pub(super) struct FunctionLowering<'a> {
    pub(super) checked: &'a mojito_checked::checked::CheckedProgram,
    pub(super) name: &'a str,
    pub(super) parameter_names: &'a [String],
    pub(super) parameter_types: Vec<Ty>,
    /// Hidden frame locals populated from compile-time value-argument registers.
    /// They are predeclared for name/type resolution but do not increase the
    /// function's ordinary runtime parameter count.
    pub(super) value_parameter_locals: Vec<(String, Ty)>,
    /// The enclosing struct's non-callable value parameters (a method), for
    /// `Self.n` bracket-slot reads; empty for a free function.
    pub(super) receiver_value_parameters: Vec<(String, Ty)>,
    /// The enclosing struct at its own binders (a static method of a
    /// value-parameterized struct), typing its receiver-less `self` slot.
    pub(super) static_receiver: Option<Ty>,
    /// The enclosing struct's binders, then the function's own.
    pub(super) enclosing_binders: EnclosingBinders,
    pub(super) owned_parameters: Vec<bool>,
    pub(super) deinit_parameters: Vec<bool>,
    pub(super) reference_parameters: Vec<bool>,
    pub(super) returns_reference: bool,
    /// Checked return type and raising contract of the declaration.
    pub(super) ret_ty: Ty,
    pub(super) raises: bool,
    pub(super) error_ty: Option<Ty>,
    pub(super) named_result: Option<&'a str>,
    /// The body's statement nodes, which HIR lowers and nested discovery
    /// walks.
    pub(super) statements: &'a [CheckedStmtId],
    /// An expression thunk's returned value, which HIR lowers after
    /// `body`.
    pub(super) returned: Option<CheckedNodeId>,
    pub(super) overloads: &'a std::sync::Arc<mojito_checked::package::ProgramOverloads>,
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
        receiver_value_parameters,
        static_receiver,
        enclosing_binders,
        owned_parameters: owned_params,
        deinit_parameters: deinit_params,
        reference_parameters: ref_params,
        returns_reference,
        ret_ty,
        raises,
        error_ty,
        named_result,
        statements,
        returned,
        overloads,
    } = request;
    let children = analyze_root_children(checked, name, statements);
    let registry = scope_registry(&children, &HashMap::new());

    let mut cfg = returned.map_or_else(
        || Cfg::build_checked_fn(checked, param_names, statements),
        |value| Cfg::build_checked_thunk(checked, param_names, statements, value),
    );
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
    // A named result's slot holds the declared result type, whether or not
    // the body assigns it whole.
    if let Some(slot) =
        named_result.and_then(|result| cfg.vars.iter().position(|candidate| candidate == result))
    {
        cfg.var_types.insert(slot as VarId, ret_ty.clone());
    }
    let mut thunks = ComptimeThunks::for_owner(name);
    let mut f = lower_cfg_nested(
        &cfg,
        &registry,
        overloads,
        returns_reference,
        &ref_params,
        &[],
        &receiver_value_parameters,
        static_receiver.as_ref(),
        &enclosing_binders,
        &mut thunks,
    );
    if !enclosing_binders.lifted {
        thunks.request_applications(checked, &cfg.body);
    }
    thunks.lower(checked, overloads, out, declarations);
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
        lower_nested_node(
            checked,
            child,
            &registry,
            overloads,
            out,
            declarations,
            Enclosing {
                name,
                binders: &enclosing_binders,
                locals: &value_parameter_locals,
            },
        );
    }
}

/// Collect the declarations belonging to one lexical function scope (those in
/// the body, its control-flow blocks, and its kept compile-time regions),
/// without descending into another declaration's body, each with the
/// `comptime for` statements of this scope enclosing it, outermost first.
/// Recursive lifting calls this once per lexical scope of statement nodes
/// `ids`.
fn find_nested_defs(
    tables: &mojito_checked::checked::CheckedTables,
    ids: &[CheckedStmtId],
    loops: &mut Vec<CheckedStmtId>,
    out: &mut Vec<NestedSite>,
) {
    for id in ids {
        let node = tables
            .statement(*id)
            .expect("a body statement id names a node of its tables");
        // Lambda expressions anywhere in this statement's own expressions
        // (conditions, iterables, values, arguments) declare hidden nested
        // definitions in this scope.
        out.extend(
            lambda_defs(tables, node)
                .into_iter()
                .map(|node| NestedSite {
                    node,
                    loops: loops.clone(),
                }),
        );
        let bodies = match &node.op {
            CheckedStmtOp::Def(_) => {
                out.push(NestedSite {
                    node: *id,
                    loops: loops.clone(),
                });
                continue;
            }
            CheckedStmtOp::If { .. }
            | CheckedStmtOp::ComptimeIf { .. }
            | CheckedStmtOp::While { .. }
            | CheckedStmtOp::For { .. }
            | CheckedStmtOp::ComptimeFor { .. }
            | CheckedStmtOp::Scope(_)
            | CheckedStmtOp::Try { .. } => node.op.bodies(),
            _ => continue,
        };
        let region = matches!(node.op, CheckedStmtOp::ComptimeFor { .. });
        if region {
            loops.push(*id);
        }
        for ids in bodies {
            find_nested_defs(tables, ids, loops, out);
        }
        if region {
            loops.pop();
        }
    }
}

/// The hidden `def` of each lambda among statement `node`'s own value
/// expressions ([`statement_lambda_operands`]); a lambda inside a lambda is
/// the inner scope's.
fn lambda_defs(
    tables: &mojito_checked::checked::CheckedTables,
    node: &mojito_checked::checked::CheckedStmt,
) -> Vec<CheckedStmtId> {
    let mut defs = Vec::new();
    for expression in statement_lambda_operands(&node.op) {
        tables.walk_expression(expression, &mut |walked| match walked {
            Walked::Expression(mojito_checked::checked::CheckedExpr {
                op: CheckedOp::Lambda { def },
                ..
            }) => {
                defs.push(*def);
                WalkResult::SkipBodies
            }
            _ => WalkResult::Advance,
        });
    }
    defs
}

/// The expressions of statement `op` whose lambdas declare nested
/// functions in its scope: the statement's own value expressions, which
/// for a module `comptime` is its value alone. A declaration's parameter
/// defaults and constraints declare none.
fn statement_lambda_operands(op: &CheckedStmtOp) -> Vec<CheckedNodeId> {
    match op {
        CheckedStmtOp::Def(_) | CheckedStmtOp::Struct(_) | CheckedStmtOp::Trait(_) => Vec::new(),
        CheckedStmtOp::Comptime { value, .. } => vec![*value],
        _ => op.expressions(),
    }
}

/// A declaration [`find_nested_defs`] found: its statement node, and the
/// `comptime for` statements of its scope enclosing it, outermost first.
struct NestedSite {
    node: CheckedStmtId,
    loops: Vec<CheckedStmtId>,
}

/// One declaration in the lexical closure tree. Checked capture facts already
/// include any ancestor forwarding required across intermediate functions.
struct NestedNode {
    node: CheckedStmtId,
    /// The indices of the `comptime for` loops of the parent's body that
    /// enclose the declaration, outermost first: the binders of the regions
    /// it is declared in.
    loop_indices: Vec<LoopIndex>,
    binding: mojito_types::origin::OwnerId,
    source_name: String,
    mangled: String,
    captures: Vec<NestedCapture>,
    callable_ty: Option<Ty>,
    /// Each parameter's checked default node.
    defaults: Vec<Option<CheckedNodeId>>,
    children: Vec<Self>,
}

impl NestedNode {
    /// The binders of the regions the declaration is declared in.
    fn region_binders(&self) -> Vec<ParamDecl> {
        self.loop_indices
            .iter()
            .map(|index| index.declaration.clone())
            .collect()
    }

    /// The checked default node of the parameter at `index`.
    fn default(&self, index: usize) -> Option<CheckedNodeId> {
        self.defaults.get(index).copied().flatten()
    }
}

fn analyze_node(
    checked: &mojito_checked::checked::CheckedProgram,
    site: NestedSite,
    parent_mangled: &str,
    duplicate_name: bool,
) -> NestedNode {
    let NestedSite { node, loops } = site;
    let statement = checked
        .tables()
        .statement(node)
        .expect("a nested def names a node of its tables");
    let (declaration, def) = checked_nested_declaration(checked, node);
    let name = &def.name;
    let binding = declaration
        .binding
        .expect("checked nested function has no value binding");

    let mangled = if duplicate_name {
        mojito_symbol::symbol::nested_lifted_declaration_name(parent_mangled, name, declaration.id)
    } else {
        mojito_symbol::symbol::nested_lifted_name(parent_mangled, name)
    };
    let captures = analyze_captures(declaration);

    let children = analyze_root_children(checked, &mangled, &def.body);

    NestedNode {
        node,
        loop_indices: loops
            .iter()
            .filter_map(|id| loop_index(checked, *id))
            .collect(),
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
        defaults: def.params.iter().map(|param| param.default).collect(),
        children,
    }
}

/// Build the callable registry visible in one lexical function. Direct
/// declarations and inherited callables are keyed by checked identity, so
/// same-spelled bindings coexist without shadow-removal heuristics.
fn scope_registry(
    children: &[NestedNode],
    inherited: &HashMap<mojito_types::origin::OwnerId, NestedInfo>,
) -> HashMap<mojito_types::origin::OwnerId, NestedInfo> {
    let mut registry = inherited.clone();
    for info in registry.values_mut() {
        info.materialized_here = false;
    }
    for child in children {
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

/// The declaration a nested function is lowered inside: its lowered name,
/// the binders in scope there, and the frame locals of its callable and
/// constructible type binders and of its own enclosing declarations', which
/// the nested body reads as its own.
#[derive(Clone, Copy)]
struct Enclosing<'a> {
    name: &'a str,
    binders: &'a EnclosingBinders,
    locals: &'a [(String, Ty)],
}

impl Enclosing<'_> {
    /// The frame locals of a body nested here declaring `decls`: its own,
    /// then each inherited one its own do not shadow.
    fn frame_locals(&self, decls: &[ParamDecl]) -> Vec<(String, Ty)> {
        let mut locals = callable_parameter_locals(decls);
        let inherited: Vec<_> = self
            .locals
            .iter()
            .filter(|(name, _)| !locals.iter().any(|(own, _)| own == name))
            .cloned()
            .collect();
        locals.extend(inherited);
        locals
    }
}

fn lower_nested_node(
    checked: &mojito_checked::checked::CheckedProgram,
    node: NestedNode,
    inherited_registry: &HashMap<mojito_types::origin::OwnerId, NestedInfo>,
    overloads: &std::sync::Arc<mojito_checked::package::ProgramOverloads>,
    out: &mut Vec<(String, MirFunction)>,
    declarations: &mut MirDeclarations,
    enclosing: Enclosing<'_>,
) {
    let ds = checked
        .tables()
        .statement(node.node)
        .expect("a nested def names a node of its tables");
    let def = def_of(checked, node.node);
    let dparams = &def.params;
    let (positional_only, keyword_only) = (&def.positional_only, &def.keyword_only);
    let registry = scope_registry(&node.children, inherited_registry);
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
    let value_parameter_locals = enclosing.frame_locals(&param_decls);
    let enclosing_binders = &enclosing
        .binders
        .with_loops(&node.loop_indices)
        .with(&param_decls);
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
    let collector_ty = |index| {
        checked
            .checked_type_at(&AnnotationSite::FunctionParam {
                module: ds.module.clone(),
                declaration: ds.span,
                syntax: ds.syntax_id,
                param: index,
            })
            .cloned()
    };
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
        .unwrap_or_else(Ty::none);
    let effect = checked
        .declaration_effect_at(&return_site)
        .cloned()
        .unwrap_or(mojito_checked::checked::DeclarationEffect {
            raises: false,
            error: None,
            returns_reference: false,
            param_writes: Vec::new(),
            availability: Vec::new(),
        });
    let capture_count = captures.len();
    let binders = enclosing_binders.with(&param_decls);
    let defaults: Vec<_> = std::iter::repeat_n(None, capture_count)
        .chain(regular.iter().enumerate().map(|(slot, (i, parameter))| {
            lower_default(
                DefaultLowering {
                    checked,
                    overloads,
                    owner: mangled,
                    parameter,
                    default: node.default(*i),
                    binders: &binders,
                    ty: declaration_types.get(capture_count + slot),
                },
                out,
                declarations,
            )
        }))
        .collect();
    declarations.functions.push(MirFunctionDeclaration {
        lowered_name: mangled.clone(),
        param_names: declaration_names,
        param_types: declaration_types,
        defaults,
        required: std::iter::repeat_n(true, capture_count)
            .chain(
                regular
                    .iter()
                    .map(|(_, parameter)| parameter.default.is_none()),
            )
            .collect(),
        variadic: variadic_idx.map(|index| collector_ty(index).unwrap_or_else(Ty::none)),
        variadic_convention: variadic_idx.and_then(|index| dparams[index].convention),
        variadic_index: runtime_variadic_index(dparams, variadic_idx)
            .map(|index| capture_count + index),
        kw_variadic: kw_variadic_idx.map(|index| collector_ty(index).unwrap_or_else(Ty::none)),
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
        // Captured places stay exclusive at the call.
        param_writes: std::iter::repeat_n(true, captures.len())
            .chain(effect.param_writes.iter().copied())
            .collect(),
        availability: Vec::new(),
        enclosing: Some(enclosing.name.to_string()),
        region_binders: node.region_binders(),
    });
    let immutable_captures: HashSet<String> = captures
        .iter()
        .filter(|capture| capture.kind == mojito_ast::ast::CaptureKind::Imm)
        .map(|capture| capture.name.clone())
        .collect();
    let mut ncfg =
        Cfg::build_checked_fn_with_captures(checked, &names, immutable_captures, &def.body);
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
    let mut thunks = ComptimeThunks::for_owner(mangled);
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
        &[],
        None,
        enclosing_binders,
        &mut thunks,
    );
    thunks.request_applications(checked, &ncfg.body);
    thunks.lower(checked, overloads, out, declarations);
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
        nf.error_ty.clone_from(&effect.error);
    }
    if let Some(result) = named_result {
        materialize_named_result_return(&mut nf, &result.name);
    }
    out.push((mangled.clone(), nf));
    for child in node.children {
        lower_nested_node(
            checked,
            child,
            &registry,
            overloads,
            out,
            declarations,
            Enclosing {
                name: mangled,
                binders: enclosing_binders,
                locals: &value_parameter_locals,
            },
        );
    }
}

/// Return the named `out` result at every exit that returns nothing else:
/// the result is transferred to the caller, so no destructor runs on it
/// here.
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
                mode: UseMode::Move,
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
    if let Some(TyKind::RuntimePack(elements)) = ty
        .callable()
        .and_then(|func| func.variadic.as_ref())
        .map(Ty::kind)
    {
        let storage = Ty::new(TyKind::Tuple(elements.clone()));
        ty.update(|kind| {
            if let Some(func) = kind.callable_mut() {
                func.variadic = Some(storage);
            }
        });
    }
    ty
}

/// Copy the checker-resolved capture environment. This includes unused explicit
/// entries and any ancestor forwarding selected by enclosing capture policies.
fn analyze_captures(
    declaration: &mojito_checked::checked::CheckedDeclaration,
) -> Vec<NestedCapture> {
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

/// The checked declaration of the nested `def` at statement node `node`,
/// and its node's `def`.
fn checked_nested_declaration(
    checked: &mojito_checked::checked::CheckedProgram,
    node: CheckedStmtId,
) -> (
    &mojito_checked::checked::CheckedDeclaration,
    &mojito_checked::checked::CheckedDef,
) {
    let declaration = checked
        .tables()
        .statement(node)
        .and_then(|node| node.declaration)
        .and_then(|id| checked.declaration(id))
        .expect("checked nested function declaration is missing");
    let def = def_of(checked, node);
    debug_assert!(
        declaration.kind == mojito_checked::checked::CheckedDeclKind::Function
            && declaration.name == def.name
    );
    (declaration, def)
}

/// The `def` of the nested declaration at statement node `node`.
fn def_of(
    checked: &mojito_checked::checked::CheckedProgram,
    node: CheckedStmtId,
) -> &mojito_checked::checked::CheckedDef {
    match checked.tables().statement(node).map(|node| &node.op) {
        Some(CheckedStmtOp::Def(def)) => def,
        _ => unreachable!("nested tree contains only def statements"),
    }
}

fn analyze_root_children(
    checked: &mojito_checked::checked::CheckedProgram,
    parent_mangled: &str,
    ids: &[CheckedStmtId],
) -> Vec<NestedNode> {
    let mut direct = Vec::new();
    find_nested_defs(checked.tables(), ids, &mut Vec::new(), &mut direct);
    let mut totals = HashMap::<&str, usize>::new();
    for site in &direct {
        *totals
            .entry(def_of(checked, site.node).name.as_str())
            .or_default() += 1;
    }
    direct
        .into_iter()
        .map(|site| {
            let duplicate = totals
                .get(def_of(checked, site.node).name.as_str())
                .copied()
                .unwrap_or_default()
                > 1;
            analyze_node(checked, site, parent_mangled, duplicate)
        })
        .collect()
}

/// The index a `comptime for` statement binds, as the CFG of its body sees
/// it ([`LoopIndex::of`]).
fn loop_index(
    checked: &mojito_checked::checked::CheckedProgram,
    id: CheckedStmtId,
) -> Option<LoopIndex> {
    let node = checked.tables().statement(id)?;
    let mojito_checked::checked::CheckedStmtOp::ComptimeFor { iter, .. } = &node.op else {
        return None;
    };
    let iteration = checked
        .expression(*iter)?
        .adjustments
        .iter()
        .find_map(|adjustment| match adjustment {
            mojito_checked::checked::SemanticAdjustment::ComptimeIteration(iteration) => {
                Some(iteration)
            }
            _ => None,
        })?;
    Some(LoopIndex::of(iteration, node.binding))
}

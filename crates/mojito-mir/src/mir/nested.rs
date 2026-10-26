//! Recursive nested-function capture discovery and MIR lifting.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

// --- Nested `def` (closure) lifting -----------------------------------------

/// Lower a function body (`name` its registered/mangled name) plus every nested
/// `def` it defines, pushing the function and each lifted nested function into
/// `out`. A liftable nested `def` becomes `name$inner`, taking its closure's
/// storage struct as its leading parameter, or, for a lambda or a
/// `@parameter` closure, its captured enclosing locals as leading reference
/// parameters; a nested `def` we can't lift stays a clean `Unsupported` at
/// execution.
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
    invariant_errors: &mut Vec<String>,
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
    let registry = scope_registry(
        &children,
        &HashMap::new(),
        &enclosing_binders,
        declarations,
        invariant_errors,
    );

    let cfg = returned.map_or_else(
        || Cfg::build_checked_fn(checked, param_names, statements),
        |value| Cfg::build_checked_thunk(checked, param_names, statements, value),
    );
    // A frame's declaration types its parameter, value-parameter, and named
    // result slots; a named result holds the declared result type whether or
    // not the body assigns it whole.
    let mut declared: HashMap<VarId, Ty> = param_types
        .iter()
        .enumerate()
        .map(|(slot, ty)| (slot as VarId, ty.clone()))
        .collect();
    for (name, ty) in &value_parameter_locals {
        let slot = cfg
            .vars
            .iter()
            .position(|candidate| candidate == name)
            .expect("value parameter was seeded into the function CFG");
        declared.insert(slot as VarId, ty.clone());
    }
    if let Some(slot) =
        named_result.and_then(|result| cfg.vars.iter().position(|candidate| candidate == result))
    {
        declared.insert(slot as VarId, ret_ty.clone());
    }
    let mut thunks = ComptimeThunks::for_owner(name);
    let mut f = lower_cfg_nested(
        &cfg,
        declared,
        &registry,
        overloads,
        returns_reference,
        &ref_params,
        &CaptureSlots::default(),
        &receiver_value_parameters,
        static_receiver.as_ref(),
        &enclosing_binders,
        &mut thunks,
    );
    if !enclosing_binders.lifted {
        thunks.request_applications(checked, &cfg.body);
    }
    thunks.lower(checked, overloads, out, declarations, invariant_errors);
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
            overloads,
            out,
            declarations,
            invariant_errors,
            Enclosing {
                name,
                registry: &registry,
                binders: &enclosing_binders,
                locals: &value_parameter_locals,
            },
        );
    }
}

/// `RuntimePack` distinguishes per-position variadic checking but is not a
/// runtime value type. A nested direct call is lowered through a closure-value
/// register, so erase that declaration-only discriminator to the Tuple storage
/// actually carried by the lifted function's single collector slot.
pub(super) fn mir_nested_callable_ty(mut ty: Ty) -> Ty {
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
    for id in tables.live_statements(ids) {
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
                    lambda: true,
                }),
        );
        let bodies = match &node.op {
            CheckedStmtOp::Def(_) => {
                out.push(NestedSite {
                    node: *id,
                    loops: loops.clone(),
                    lambda: false,
                });
                continue;
            }
            CheckedStmtOp::If { .. }
            | CheckedStmtOp::ComptimeIf { .. }
            | CheckedStmtOp::While { .. }
            | CheckedStmtOp::For { .. }
            | CheckedStmtOp::ComptimeFor { .. }
            | CheckedStmtOp::Match { .. }
            | CheckedStmtOp::ComptimeMatch { .. }
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

/// A declaration [`find_nested_defs`] found: its statement node, the
/// `comptime for` statements of its scope enclosing it, outermost first,
/// and whether it is a lambda's hidden `def`.
struct NestedSite {
    node: CheckedStmtId,
    loops: Vec<CheckedStmtId>,
    lambda: bool,
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
    /// Whether a lambda expression declares it, which makes its closure
    /// where it stands rather than at a declaration statement.
    lambda: bool,
    /// Whether it declares a capture list ([`CheckedDef::capture_list`]).
    ///
    /// [`CheckedDef::capture_list`]: mojito_checked::checked::CheckedDef::capture_list
    capture_list: bool,
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
    let NestedSite {
        node,
        loops,
        lambda,
    } = site;
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
    let captures = analyze_captures(def);

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
        lambda,
        capture_list: def.capture_list,
        defaults: def.params.iter().map(|param| param.default).collect(),
        children,
    }
}

/// Build the callable registry visible in one lexical function, whose
/// direct nested declarations are `children`. Direct declarations and inherited
/// callables are keyed by checked identity, so same-spelled bindings
/// coexist without shadow-removal heuristics. Every direct declaration
/// with a non-empty capture list builds its storage struct
/// ([`NestedInfo::storage`]), as upstream's `constructClosure` emits a
/// closure instance for every one; one without storage a sibling
/// captures keeps its closure in a slot ([`NestedInfo::slotted`]). A lambda
/// makes its closure where it stands, and a `@parameter` closure, which
/// captures without a list, takes its captures as leading reference
/// parameters. A declaration whose storage names a
/// binder no enclosing declaration binds is an invariant failure,
/// recorded in `invariant_errors`.
fn scope_registry(
    children: &[NestedNode],
    inherited: &HashMap<mojito_types::origin::OwnerId, NestedInfo>,
    binders: &EnclosingBinders,
    declarations: &MirDeclarations,
    invariant_errors: &mut Vec<String>,
) -> HashMap<mojito_types::origin::OwnerId, NestedInfo> {
    let mut registry = inherited.clone();
    for info in registry.values_mut() {
        info.materialized_here = false;
    }
    // A sibling's storage is the referent of a field capturing it, and a
    // body captures only siblings declared before it.
    for child in children {
        let captured_by_sibling = children.iter().any(|sibling| {
            sibling.binding != child.binding
                && sibling
                    .captures
                    .iter()
                    .any(|capture| capture.binding == child.binding)
        });
        let storage_backed = !child.lambda && child.capture_list && !child.captures.is_empty();
        let storage = storage_backed
            .then(|| closure_storage(child, &registry, binders, declarations))
            .flatten();
        if storage_backed && storage.is_none() {
            invariant_errors.push(format!(
                "nested def `{}`: its closure storage names a parameter no enclosing declaration binds",
                child.source_name
            ));
        }
        registry.insert(
            child.binding,
            NestedInfo {
                binding: child.binding,
                source_name: child.source_name.clone(),
                mangled: child.mangled.clone(),
                materialized_here: true,
                slotted: captured_by_sibling && storage.is_none(),
                storage,
                captures: child.captures.clone(),
                callable_ty: child.callable_ty.clone(),
            },
        );
    }
    registry
}

/// The storage struct of the capture-list `def` `child`, whose
/// captured siblings `registry` registers, declared where `binders` are in
/// scope: its parameters are the binders its fields name, as upstream
/// promotes the captures' parameters to the storage's own. `None` for a
/// body without captures, whose closure is a function value, and for one
/// whose fields name a parameter no binder declares.
fn closure_storage(
    child: &NestedNode,
    registry: &HashMap<mojito_types::origin::OwnerId, NestedInfo>,
    binders: &EnclosingBinders,
    declarations: &MirDeclarations,
) -> Option<ClosureStorage> {
    if child.captures.is_empty() {
        return None;
    }
    let mut writes = false;
    let (mut register_passable, mut trivial) = (true, true);
    let mut meet = |passable: bool, identity: bool| {
        register_passable &= passable;
        trivial &= identity;
    };
    let fields: Vec<(String, Ty)> = child
        .captures
        .iter()
        .map(|capture| {
            let ty = match capture.kind {
                mojito_ast::ast::CaptureKind::Copy | mojito_ast::ast::CaptureKind::Move => {
                    writes = true;
                    // A copy of a reference binding copies its referent.
                    let ty = capture_referent(capture);
                    meet(
                        declarations.register_passable(&ty),
                        declarations.copy_is_identity(&ty),
                    );
                    ty
                }
                mojito_ast::ast::CaptureKind::Imm
                | mojito_ast::ast::CaptureKind::Mut
                | mojito_ast::ast::CaptureKind::Ref => {
                    let (referent, mutable, passable, identity) =
                        captured_storage(registry, capture).map_or_else(
                            || {
                                let referent = capture_referent(capture);
                                let passable = declarations.register_passable(&referent);
                                let identity = declarations.copy_is_identity(&referent);
                                (
                                    referent,
                                    capture.kind != mojito_ast::ast::CaptureKind::Imm,
                                    passable,
                                    identity,
                                )
                            },
                            |storage| {
                                (
                                    storage.ty.clone(),
                                    storage.writes,
                                    storage.register_passable,
                                    storage.trivial,
                                )
                            },
                        );
                    meet(passable, identity);
                    writes |= mutable;
                    TyKind::Ref(mojito_types::origin::RefTy {
                        referent,
                        origin: mojito_types::origin::Origin::Untracked { mutable },
                        mutability: if mutable {
                            mojito_types::origin::Mutability::Mutable
                        } else {
                            mojito_types::origin::Mutability::Immutable
                        },
                    })
                    .into()
                }
            };
            (capture.name.clone(), ty)
        })
        .collect();
    let in_scope = binders.with(&child.region_binders());
    // A reference field names the binders its referent names.
    let contents: Vec<&Ty> = fields
        .iter()
        .map(|(_, ty)| match ty.kind() {
            TyKind::Ref(reference) => &reference.referent,
            _ => ty,
        })
        .collect();
    let names_binder = |ty: &Ty, id: &mojito_types::param_expr::ParamId| {
        mojito_types::types::mentions(
            ty,
            &|ty| matches!(ty.kind(), TyKind::Param { binder, .. } if binder.id == *id),
        )
    };
    let mut values = HashSet::new();
    for ty in &contents {
        mojito_types::types::referenced_parameters(ty, &mut values);
    }
    let param_decls: Vec<ParamDecl> = in_scope
        .declarations
        .iter()
        .filter(|declaration| {
            values.contains(declaration.name().trim_start_matches('*'))
                || contents.iter().any(|ty| names_binder(ty, declaration.id()))
        })
        .cloned()
        .collect();
    let unbound = contents.iter().any(|ty| {
        mojito_types::types::mentions(ty, &|ty| {
            matches!(ty.kind(), TyKind::Param { binder, .. }
                if !param_decls.iter().any(|declaration| *declaration.id() == binder.id))
        })
    }) || values.iter().any(|name| {
        !param_decls
            .iter()
            .any(|declaration| declaration.name().trim_start_matches('*') == name)
    });
    if unbound {
        return None;
    }
    Some(ClosureStorage {
        ty: TyKind::Struct(
            mojito_symbol::symbol::closure_storage_name(&child.mangled),
            param_decls.iter().map(ParamDecl::own_argument).collect(),
        )
        .into(),
        fields,
        param_decls,
        writes,
        register_passable,
        trivial,
    })
}

/// The storage of the sibling closure `capture` captures, if it has one.
fn captured_storage<'r>(
    registry: &'r HashMap<mojito_types::origin::OwnerId, NestedInfo>,
    capture: &NestedCapture,
) -> Option<&'r ClosureStorage> {
    registry.get(&capture.binding)?.storage.as_ref()
}

/// The type a reference capture names: the referent of a captured
/// reference binding (a loop's reference target), else the capture's own.
fn capture_referent(capture: &NestedCapture) -> Ty {
    match capture.ty.kind() {
        TyKind::Ref(reference) => reference.referent.clone(),
        _ => mir_nested_callable_ty(capture.ty.clone()),
    }
}

/// Push the declaration of the storage struct `storage` of the closure
/// whose `captures` its fields hold, and its synthesized members: the
/// fieldwise `__init__` the declaration statement calls, as upstream's
/// `ClosureEmitter` synthesizes it, reading a `var` capture, taking a
/// moved one, and storing a reference to a reference capture's referent,
/// and a `__deinit__` when a field has a destructor to run.
fn lower_closure_storage(
    storage: &ClosureStorage,
    captures: &[NestedCapture],
    functions: &mut Vec<(String, MirFunction)>,
    declarations: &mut MirDeclarations,
) {
    let TyKind::Struct(name, _) = storage.ty.kind() else {
        unreachable!("a closure's storage is a struct")
    };
    let parameter_names: Vec<String> = captures
        .iter()
        .map(|capture| match capture.name.as_str() {
            "self" => "__capture_self".to_string(),
            name => name.to_string(),
        })
        .collect();
    let parameters: Vec<FieldwiseParameter<'_>> = storage
        .fields
        .iter()
        .zip(captures)
        .zip(&parameter_names)
        .map(|(((field, ty), capture), parameter)| FieldwiseParameter {
            field,
            parameter,
            ty: ty.clone(),
            convention: match capture.kind {
                mojito_ast::ast::CaptureKind::Copy => None,
                mojito_ast::ast::CaptureKind::Move => Some(ArgConvention::Var),
                mojito_ast::ast::CaptureKind::Imm
                | mojito_ast::ast::CaptureKind::Mut
                | mojito_ast::ast::CaptureKind::Ref => Some(ArgConvention::Ref),
            },
        })
        .collect();
    let param_writes = parameters
        .iter()
        .map(|parameter| match parameter.ty.kind() {
            TyKind::Ref(reference) => {
                reference.mutability == mojito_types::origin::Mutability::Mutable
            }
            _ => is_owned(&parameter.convention),
        })
        .collect();
    lower_fieldwise_members(
        (name, &storage.param_decls),
        &parameters,
        param_writes,
        functions,
        declarations,
    );
    let destroys = !storage
        .fields
        .iter()
        .all(|(_, ty)| declarations.drop_runs_no_code(ty));
    if destroys {
        lower_synthesized_member(
            (name, &[]),
            &storage.fields,
            SynthesizedMember::Destroy,
            functions,
            declarations,
        );
    }
    declarations.structs.push(MirStructDeclaration {
        name: name.clone(),
        fields: storage.fields.clone(),
        mut_self_methods: HashSet::new(),
        fieldwise_init: true,
        param_decls: storage.param_decls.clone(),
        explicit_destructors: HashMap::new(),
        explicit_destroy: None,
        conformances: Vec::new(),
        declared: ["Movable", "Deinitable"]
            .into_iter()
            .map(|trait_name| mojito_checked::checked::StructConformance {
                trait_name: trait_name.to_string(),
                conditions: Vec::new(),
            })
            .chain(
                [
                    (storage.trivial, "TrivialRegisterPassable"),
                    (storage.register_passable, "RegisterPassable"),
                ]
                .into_iter()
                .filter(|(holds, _)| *holds)
                .map(|(_, trait_name)| {
                    mojito_checked::checked::StructConformance {
                        trait_name: trait_name.to_string(),
                        conditions: vec![GenericConstraint::Bool(true)],
                    }
                }),
            )
            .collect(),
        associated_types: Vec::new(),
        parameterized_associated_types: Vec::new(),
        associated_values: Vec::new(),
        parameterized_associated_values: Vec::new(),
        witness_tables: Vec::new(),
        members: Vec::new(),
    });
}

/// The declaration a nested function is lowered inside: its lowered name,
/// the nested `def`s registered in its scope, the binders in scope there,
/// and the frame locals of its callable and constructible type binders and
/// of its own enclosing declarations', which the nested body reads as its
/// own.
#[derive(Clone, Copy)]
struct Enclosing<'a> {
    name: &'a str,
    registry: &'a HashMap<mojito_types::origin::OwnerId, NestedInfo>,
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
    overloads: &std::sync::Arc<mojito_checked::package::ProgramOverloads>,
    out: &mut Vec<(String, MirFunction)>,
    declarations: &mut MirDeclarations,
    invariant_errors: &mut Vec<String>,
    enclosing: Enclosing<'_>,
) {
    let def = def_of(checked, node.node);
    let dparams = &def.params;
    let (positional_only, keyword_only) = (&def.positional_only, &def.keyword_only);
    let signature = &def.signature;
    let param_decls = signature.parameters.clone();
    let enclosing_binders = &enclosing
        .binders
        .with_loops(&node.loop_indices)
        .with(&param_decls);
    let registry = scope_registry(
        &node.children,
        enclosing.registry,
        enclosing_binders,
        declarations,
        invariant_errors,
    );
    let captures = &node.captures;
    let mangled = &node.mangled;
    let storage = enclosing
        .registry
        .get(&node.binding)
        .and_then(|info| info.storage.clone());
    if let Some(storage) = &storage {
        lower_closure_storage(storage, captures, out, declarations);
    }
    // A reference capture of a reference binding (a loop's reference
    // target) is a reference parameter, typed by its referent as any is,
    // and a capture of a sibling closure with storage names that storage.
    let capture_types: Vec<Ty> = captures
        .iter()
        .map(|capture| {
            if let Some(storage) = captured_storage(enclosing.registry, capture) {
                return storage.ty.clone();
            }
            match capture.kind {
                mojito_ast::ast::CaptureKind::Imm
                | mojito_ast::ast::CaptureKind::Mut
                | mojito_ast::ast::CaptureKind::Ref => capture_referent(capture),
                _ => mir_nested_callable_ty(capture.ty.clone()),
            }
        })
        .collect();
    // An `imm` capture is an immutable borrow at the call; a `mut` or
    // `ref` one may write its place, and so may a call of a captured
    // sibling through storage it writes.
    let capture_writes: Vec<bool> = captures
        .iter()
        .map(|capture| {
            captured_storage(enclosing.registry, capture).map_or(
                capture.kind != mojito_ast::ast::CaptureKind::Imm,
                |storage| storage.writes,
            )
        })
        .collect();
    let named_result = dparams
        .iter()
        .find(|parameter| matches!(parameter.convention, Some(ArgConvention::Out)));
    let caller_params: Vec<_> = dparams
        .iter()
        .enumerate()
        .filter(|(_, parameter)| !matches!(parameter.convention, Some(ArgConvention::Out)))
        .collect();
    // A body with storage takes it as its one leading `read` parameter,
    // as upstream's lifted method takes `self`, and holds its captures in
    // frame locals; a lambda's body takes each capture as a leading
    // reference parameter.
    let capture_names: Vec<String> = captures
        .iter()
        .map(|capture| capture.name.clone())
        .collect();
    let (leading_names, leading_types, leading_writes) = match &storage {
        Some(storage) => storage_parameter(storage, captures),
        None => (capture_names.clone(), capture_types, capture_writes.clone()),
    };
    let leading = leading_names.len();
    let mut names = leading_names.clone();
    names.extend(
        caller_params
            .iter()
            .map(|(_, parameter)| parameter.name.clone()),
    );
    if let Some(result) = named_result {
        names.push(result.name.clone());
    }
    let value_parameter_locals = enclosing.frame_locals(&param_decls);
    names.extend(value_parameter_locals.iter().map(|(name, _)| name.clone()));
    let capture_locals = names.len();
    if storage.is_some() {
        names.extend(capture_names);
    }
    let mut ptys = leading_types.clone();
    ptys.extend(caller_params.iter().map(|(param, p)| {
        let ty = signature.param(*param).cloned().unwrap_or_else(|| {
            panic!(
                "checked type missing for parameter '{}' of nested function",
                p.name
            )
        });
        body_parameter_ty(p, ty)
    }));
    // The closure object owns declaration-time copy/move captures. A lifted
    // invocation only borrows its storage or those environment slots, just
    // as it borrows explicit read/mut/ref captures; it must neither destroy
    // nor duplicate them on each call.
    let mut owned2: Vec<bool> = vec![false; leading];
    owned2.extend(
        caller_params
            .iter()
            .map(|(_, parameter)| is_owned(&parameter.convention)),
    );
    // Captures are borrowed, never `deinit` sources.
    let mut deinit2: Vec<bool> = vec![false; leading];
    deinit2.extend(
        caller_params
            .iter()
            .map(|(_, parameter)| is_deinit(&parameter.convention)),
    );
    // A lambda's captures are `mut` (their final value is written back to
    // the enclosing variable — reference-capture semantics); a storage is
    // read.
    let mut refp2: Vec<bool> = vec![storage.is_none(); leading];
    refp2.extend(
        caller_params
            .iter()
            .map(|(_, parameter)| is_ref(&parameter.convention)),
    );
    let collector_ty = |index| signature.param(index).cloned();
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
    let mut declaration_names = leading_names;
    declaration_names.extend(regular.iter().map(|(_, parameter)| parameter.name.clone()));
    let mut declaration_types = leading_types;
    declaration_types.extend(regular.iter().map(|(index, parameter)| {
        signature.param(*index).cloned().unwrap_or_else(|| {
            panic!(
                "checked type missing for parameter '{}' of nested function",
                parameter.name
            )
        })
    }));
    let return_ty = signature.result.clone().unwrap_or_else(Ty::none);
    let effect = signature.effect.clone().unwrap_or_default();
    let capture_count = leading;
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
                invariant_errors,
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
        param_conventions: std::iter::repeat_n(
            storage.is_none().then_some(ArgConvention::Ref),
            capture_count,
        )
        .chain(regular.iter().map(|(_, parameter)| parameter.convention))
        .collect(),
        ret_ty: return_ty,
        returns_reference: effect.returns_reference,
        raises: effect.raises,
        error_ty: effect.error,
        ref_params: std::iter::repeat_n(storage.is_none(), capture_count)
            .chain(
                regular
                    .iter()
                    .map(|(_, parameter)| is_ref(&parameter.convention)),
            )
            .collect(),
        param_writes: leading_writes
            .into_iter()
            .chain(effect.param_writes.iter().copied())
            .collect(),
        availability: Vec::new(),
        enclosing: Some(enclosing.name.to_string()),
        region_binders: node.region_binders(),
        inline: def.inline,
        provenance: mojito_checked::declaration::FunctionProvenance::Source,
    });
    let immutable_captures: HashSet<String> = captures
        .iter()
        .zip(&capture_writes)
        .filter(|(capture, writes)| capture.kind == mojito_ast::ast::CaptureKind::Imm && !**writes)
        .map(|(capture, _)| capture.name.clone())
        .collect();
    let ncfg = Cfg::build_checked_fn_with_captures(checked, &names, immutable_captures, &def.body);
    // Every lifted slot takes its checker-owned storage type. In particular,
    // an intermediate closure may only forward a captured sibling closure to
    // a descendant, so no op in its body declares that callable type.
    let mut declared: HashMap<VarId, Ty> = ptys
        .iter()
        .enumerate()
        .map(|(slot, ty)| (slot as VarId, ty.clone()))
        .collect();
    for (name, ty) in &value_parameter_locals {
        let slot = ncfg
            .vars
            .iter()
            .position(|candidate| candidate == name)
            .expect("nested value parameter was seeded into the function CFG");
        declared.insert(slot as VarId, ty.clone());
    }
    let capture_slots = capture_slots(
        captures,
        storage.as_ref(),
        if storage.is_some() { capture_locals } else { 0 },
        checked
            .tables()
            .statement(node.node)
            .expect("a nested def names a node of its tables")
            .location(),
    );
    let mut thunks = ComptimeThunks::for_owner(mangled);
    let mut nf = lower_cfg_nested(
        &ncfg,
        declared,
        &registry,
        overloads,
        effect.returns_reference,
        &refp2,
        &capture_slots,
        &[],
        None,
        enclosing_binders,
        &mut thunks,
    );
    thunks.request_applications(checked, &ncfg.body);
    thunks.lower(checked, overloads, out, declarations, invariant_errors);
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
    nf.ret_ty.clone_from(&signature.result);
    if let Some(effect) = &signature.effect {
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
            overloads,
            out,
            declarations,
            invariant_errors,
            Enclosing {
                name: mangled,
                registry: &registry,
                binders: enclosing_binders,
                locals: &value_parameter_locals,
            },
        );
    }
}

/// The leading parameter of a body taking its closure's `storage`, whose
/// `captures` its fields hold: its name, `self` as upstream's lifted
/// method spells it unless a capture is, its type, and whether a call
/// writes it, which only a body owning a moved capture can.
fn storage_parameter(
    storage: &ClosureStorage,
    captures: &[NestedCapture],
) -> (Vec<String>, Vec<Ty>, Vec<bool>) {
    let name = if captures.iter().any(|capture| capture.name == "self") {
        "__closure_self"
    } else {
        "self"
    };
    let writes = captures
        .iter()
        .any(|capture| capture.kind == mojito_ast::ast::CaptureKind::Move);
    (
        vec![name.to_string()],
        vec![storage.ty.clone()],
        vec![writes],
    )
}

/// The slots `captures` bind in a lifted body, from slot `first` on: each
/// a local holding a reference to its field of `storage` when the body
/// takes it, at `location`, else each a leading reference parameter.
fn capture_slots(
    captures: &[NestedCapture],
    storage: Option<&ClosureStorage>,
    first: usize,
    location: SourceSpan,
) -> CaptureSlots {
    let slot = |index: usize| (first + index) as VarId;
    CaptureSlots {
        owners: captures
            .iter()
            .enumerate()
            .map(|(index, capture)| (capture.binding, slot(index)))
            .collect(),
        storage: storage.map(|storage| CaptureStorage {
            slot: 0,
            ty: storage.ty.clone(),
            fields: captures
                .iter()
                .zip(&storage.fields)
                .enumerate()
                .map(|(index, (capture, (field, ty)))| {
                    (
                        slot(index),
                        field.clone(),
                        ty.clone(),
                        capture.kind == mojito_ast::ast::CaptureKind::Move,
                    )
                })
                .collect(),
            location,
        }),
    }
}

/// Return the named `out` result at every exit that returns nothing else:
/// the result is transferred to the caller, so no destructor runs on it
/// here. The exits are the function's own blocks' and those of the region
/// ops among them, never a `try` part's.
fn materialize_named_result_return(function: &mut MirFunction, result: &str) {
    let slot = function
        .var_names
        .iter()
        .position(|name| name == result)
        .expect("named result was seeded into the function variables") as VarId;
    let mut blocks = std::mem::take(&mut function.blocks);
    materialize_named_result_exits(&mut blocks, slot, &mut function.n_regs);
    function.blocks = blocks;
}

fn materialize_named_result_exits(blocks: &mut [MirBlock], slot: VarId, n_regs: &mut u32) {
    for block in blocks {
        for instruction in &mut block.instrs {
            if !matches!(instruction, MirInstr::Try { .. }) {
                for region in instruction_regions_mut(instruction) {
                    materialize_named_result_exits(region, slot, n_regs);
                }
            }
        }
        if matches!(block.term, MirTerm::Return(None) | MirTerm::TryYield(_)) {
            let register = Reg(*n_regs);
            *n_regs += 1;
            block.instrs.push(MirInstr::UseVar {
                dest: register,
                var: slot,
                mode: UseMode::Move,
                constructor: None,
            });
            block.term = MirTerm::Return(Some(register));
        }
    }
}

/// Copy the checker-resolved capture environment. This includes unused explicit
/// entries and any ancestor forwarding selected by enclosing capture policies.
fn analyze_captures(def: &mojito_checked::checked::CheckedDef) -> Vec<NestedCapture> {
    def.captures
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

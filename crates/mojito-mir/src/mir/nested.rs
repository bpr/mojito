//! Recursive nested-function capture discovery and MIR lifting.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::closure_uses::NestedDefSite;
use mojito_types::types::TyKind;

// --- Nested `def` (closure) lifting -----------------------------------------

/// Lower a function body (`name` its registered/mangled name) plus every nested
/// `def` it defines, pushing the function and each lifted nested function into
/// `out`. A liftable nested `def` takes the symbol the check gave it
/// (`name$inner` for one `name` declares), and its closure's storage
/// struct as its leading parameter, or, for a body without storage, its
/// captured enclosing locals as leading reference parameters (R1946); a
/// nested `def` we can't lift stays a clean `Unsupported` at execution.
pub(super) struct FunctionLowering<'a> {
    pub(super) checked: &'a mojito_checked::checked::CheckedProgram,
    pub(super) name: &'a str,
    pub(super) parameter_names: &'a [String],
    pub(super) parameter_types: Vec<Ty>,
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
    /// Whether the result is the `out self` receiver of an initializer,
    /// which a `return value` initializes.
    pub(super) receiver_result: bool,
    /// The body's statement nodes, which HIR lowers and nested discovery
    /// walks.
    pub(super) statements: &'a [CheckedStmtId],
    /// An expression thunk's returned value, which HIR lowers after
    /// `body`.
    pub(super) returned: Option<CheckedNodeId>,
    pub(super) overloads: &'a std::sync::Arc<mojito_checked::package::ProgramOverloads>,
    /// The nested `def`s of the declaration a lifted expression is lifted
    /// from, which it calls directly, as the pin's compile-time expression
    /// names `outer::inner` over the enclosing parameters
    /// ([`ComptimeThunks::callables`]). Empty for any other body.
    pub(super) callables: &'a HashMap<mojito_types::origin::OwnerId, NestedInfo>,
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
        enclosing_binders,
        owned_parameters: owned_params,
        deinit_parameters: deinit_params,
        reference_parameters: ref_params,
        returns_reference,
        ret_ty,
        raises,
        error_ty,
        named_result,
        receiver_result,
        statements,
        returned,
        overloads,
        callables,
    } = request;
    let starts = LoweredFrom::at(out, declarations);
    let closures = ClosureLowering::default();
    let children = analyze_root_children(checked, statements, returned);
    let registry = scope_registry(&children, callables, invariant_errors);

    let cfg = returned.map_or_else(
        || Cfg::build_checked_fn(checked, param_names, statements),
        |value| Cfg::build_checked_thunk(checked, param_names, statements, value),
    );
    // A frame's declaration types its parameter and named result slots; a
    // named result holds the declared result type whether or not the body
    // assigns it whole.
    let mut declared: HashMap<VarId, Ty> = param_types
        .iter()
        .enumerate()
        .map(|(slot, ty)| (slot as VarId, ty.clone()))
        .collect();
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
        &enclosing_binders,
        &mut thunks,
        raises.then(|| error_ty.clone().unwrap_or_else(Ty::error)),
    );
    if !enclosing_binders.lifted {
        thunks.request_applications(checked, &cfg.body);
    }
    thunks.inherit_callables(&registry);
    thunks.lower(checked, overloads, out, declarations, invariant_errors);
    f.n_params = param_types.len();
    for (slot, ty) in param_types.iter().enumerate() {
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
    if receiver_result {
        initialize_receiver_returns(&mut f);
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
                closures: &closures,
            },
        );
    }
    let moved: HashSet<String> = closures
        .storage_scopes
        .into_inner()
        .iter()
        .flat_map(|scope| own_storage_binders(scope, out, declarations))
        .collect();
    follow_moved_contracts(&moved, &mut out[starts.functions..], declarations);
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

/// Where one [`lower_fn_nested`] began pushing: the functions, function
/// declarations, and struct declarations it lowers lie past these.
#[derive(Clone, Copy)]
struct LoweredFrom {
    functions: usize,
    declarations: usize,
    structs: usize,
}

impl LoweredFrom {
    /// The end of what has been pushed to `out` and `declarations` so far.
    const fn at(out: &[(String, MirFunction)], declarations: &MirDeclarations) -> Self {
        Self {
            functions: out.len(),
            declarations: declarations.functions.len(),
            structs: declarations.structs.len(),
        }
    }
}

/// A closure storage struct whose binders its lowering declared as the
/// enclosing declarations', with the range of what was lowered under it
/// (`from` up to `to`): its struct declaration and synthesized members,
/// and, when the body is its method, the method and everything nested in
/// it. [`own_storage_binders`] declares them as the struct's own.
struct StorageScope {
    storage: String,
    /// The struct's own parameters, as the check declared them
    /// ([`mojito_checked::checked::CheckedClosureUses::parameters`]).
    parameters: Vec<ParamDecl>,
    from: LoweredFrom,
    to: LoweredFrom,
}

/// What lowering the closures declared in one [`lower_fn_nested`]
/// collects for it to resolve once they are all lowered.
#[derive(Default)]
struct ClosureLowering {
    /// The storage scopes it lowered, innermost first.
    storage_scopes: std::cell::RefCell<Vec<StorageScope>>,
}

impl ClosureLowering {
    /// Record `scope`, a storage's own lowering, extended to `method_end`
    /// when its body is its method, which owns everything lowered under
    /// it; a storage whose body is not owns its members alone.
    fn push_storage_scope(&self, scope: Option<StorageScope>, method_end: Option<LoweredFrom>) {
        if let Some(mut scope) = scope {
            if let Some(end) = method_end {
                scope.to = end;
            }
            self.storage_scopes.borrow_mut().push(scope);
        }
    }
}

/// Declare the binders of the closure storage `scope` names as the
/// struct's own, as upstream's `liftClosure` declares its storage's
/// parameters, reusing their spellings: each reference to one in what
/// was lowered under the storage moves to the struct's binder, a
/// declaration copying one (a synthesized member's, a compile-time
/// thunk's) declares the struct's instead, and a reference to a binder
/// typed by one is retyped. The storage type the enclosing body spells
/// keeps the enclosing binders as its arguments. Returns the
/// declarations moved, whose call sites [`follow_moved_contracts`]
/// updates.
fn own_storage_binders(
    scope: &StorageScope,
    out: &mut [(String, MirFunction)],
    declarations: &mut MirDeclarations,
) -> Vec<String> {
    let structs = &mut declarations.structs[scope.from.structs..scope.to.structs];
    let Some(storage) = structs
        .iter_mut()
        .find(|declaration| declaration.name == scope.storage)
    else {
        return Vec::new();
    };
    if storage.param_decls.is_empty() {
        return Vec::new();
    }
    let decls = scope.parameters.clone();
    let bindings = mojito_types::types::own_parameter_bindings(&storage.param_decls, &decls);
    let mut binders = StorageBinders {
        context: ParamContext::detached(),
        owned: storage
            .param_decls
            .iter()
            .map(ParamDecl::id)
            .cloned()
            .zip(decls.iter().cloned())
            .collect(),
        bindings,
    };
    storage.param_decls = decls;
    let moved = &mut declarations.functions[scope.from.declarations..scope.to.declarations];
    let retyped: Vec<ParamDecl> = moved
        .iter()
        .flat_map(|declaration| {
            declaration
                .param_decls
                .iter()
                .chain(&declaration.region_binders)
        })
        .filter(|decl| !binders.owned.contains_key(decl.id()))
        .filter_map(|decl| {
            let mut retyped = decl.clone();
            binders.move_decls(std::slice::from_mut(&mut retyped));
            (retyped != *decl).then_some(retyped)
        })
        .collect();
    for decl in retyped {
        match decl.own_argument() {
            TyArg::Ty(ty) => binders.bindings.bind_type(decl.id().clone(), ty),
            TyArg::Val(CtValue::Expr(reference)) => {
                binders.bindings.bind(decl.id().clone(), reference);
            }
            TyArg::Val(_) | TyArg::Origin(_) => {}
        }
    }
    for declaration in structs {
        for (_, ty) in &mut declaration.fields {
            *ty = binders.moved(ty);
        }
    }
    for (_, function) in &mut out[scope.from.functions..scope.to.functions] {
        let Ok(()) = crate::mir::visit::walk_function_mut(&mut binders, function);
    }
    for declaration in moved.iter_mut() {
        for ty in declaration
            .param_types
            .iter_mut()
            .chain(std::iter::once(&mut declaration.ret_ty))
            .chain(declaration.variadic.as_mut())
            .chain(declaration.kw_variadic.as_mut())
            .chain(declaration.error_ty.as_mut())
        {
            *ty = binders.moved(ty);
        }
        binders.move_decls(&mut declaration.param_decls);
        binders.move_decls(&mut declaration.region_binders);
        for constraint in &mut declaration.availability {
            binders.move_constraint(constraint);
        }
    }
    moved
        .iter()
        .map(|declaration| declaration.lowered_name.clone())
        .collect()
}

/// Copy into each method call in `functions` of one of the `moved`
/// declarations that declaration's compile-time parameters, which a call
/// carries as its callee declares them.
fn follow_moved_contracts(
    moved: &HashSet<String>,
    functions: &mut [(String, MirFunction)],
    declarations: &MirDeclarations,
) {
    fn follow(blocks: &mut [MirBlock], moved: &HashSet<String>, declarations: &MirDeclarations) {
        for instruction in blocks.iter_mut().flat_map(|block| &mut block.instrs) {
            if let MirInstr::MethodCall {
                resolved: Some(callee),
                param_decls,
                ..
            } = instruction
                && moved.contains(callee)
                && let Some(declaration) = declarations.function(callee)
            {
                param_decls.clone_from(&declaration.param_decls);
            }
            for region in instruction_regions_mut(instruction) {
                follow(region, moved, declarations);
            }
        }
    }
    if moved.is_empty() {
        return;
    }
    for (_, function) in functions {
        follow(&mut function.blocks, moved, declarations);
    }
}

/// The rewrite [`own_storage_binders`] applies at every site: a
/// reference to an enclosing binder the storage declares moves to the
/// storage's own, which `owned` maps it to.
struct StorageBinders {
    context: ParamContext,
    bindings: mojito_types::param_expr::ParamBindings,
    owned: HashMap<mojito_types::param_expr::ParamId, ParamDecl>,
}

impl StorageBinders {
    fn moved(&self, ty: &Ty) -> Ty {
        mojito_types::types::replace_parameters(&self.context, ty, &self.bindings, 0)
            .unwrap_or_else(|_| ty.clone())
    }

    fn moved_expr(&self, expr: &ParamExpr) -> ParamExpr {
        self.context
            .replace(expr, &self.bindings)
            .unwrap_or_else(|_| expr.clone())
    }

    fn move_constraint(&self, constraint: &mut GenericConstraint) {
        if let Ok(moved) = mojito_types::types::replace_constraint_parameters(
            &self.context,
            constraint,
            &self.bindings,
        ) {
            *constraint = moved;
        }
    }

    /// Move the references `decls` make, each keeping its identity but a
    /// copy of a binder the storage declares, which becomes the storage's.
    fn move_decls(&self, decls: &mut [ParamDecl]) {
        for decl in decls {
            if let Some(own) = self.owned.get(decl.id()) {
                decl.clone_from(own);
                continue;
            }
            match decl {
                ParamDecl::Type {
                    trait_param,
                    callable_bound,
                    default,
                    constraints,
                    ..
                } => {
                    for ty in callable_bound.iter_mut().chain(default.iter_mut()) {
                        *ty = self.moved(ty);
                    }
                    if let Some(binder) = trait_param
                        && let Some(own) = self.owned.get(&binder.id)
                    {
                        *binder = own.binder();
                    }
                    for constraint in constraints {
                        self.move_constraint(constraint);
                    }
                }
                ParamDecl::Value {
                    ty,
                    default,
                    constraints,
                    ..
                } => {
                    *ty = self.moved(ty);
                    if let Some(default) = default {
                        *default = self.moved_expr(default);
                    }
                    for constraint in constraints {
                        self.move_constraint(constraint);
                    }
                }
            }
        }
    }
}

impl crate::mir::visit::MutTypeVisitor for StorageBinders {
    type Error = std::convert::Infallible;

    fn visit_ty(&mut self, ty: &mut Ty, _: crate::mir::visit::TySite) -> Result<(), Self::Error> {
        *ty = self.moved(ty);
        Ok(())
    }

    fn visit_ty_arg(&mut self, arg: &mut TyArg) -> Result<(), Self::Error> {
        *arg = match arg {
            TyArg::Ty(ty) => TyArg::Ty(self.moved(ty)),
            _ => {
                mojito_types::types::replace_argument_parameters(&self.context, arg, &self.bindings)
                    .unwrap_or_else(|_| arg.clone())
            }
        };
        Ok(())
    }

    fn visit_variant_alternatives(
        &mut self,
        alternatives: &mut Vec<Ty>,
    ) -> Result<(), Self::Error> {
        for ty in alternatives {
            *ty = self.moved(ty);
        }
        Ok(())
    }

    fn visit_variant_index(
        &mut self,
        index: &mut mojito_types::types::VariantIndex,
    ) -> Result<(), Self::Error> {
        let Ok(()) = index
            .try_map_expr(|expr| Ok::<_, std::convert::Infallible>(self.moved_expr(expr)))
            .map(|moved| *index = moved);
        Ok(())
    }

    fn visit_field_step(&mut self, step: &mut Proj, _: Option<&Ty>) -> Result<(), Self::Error> {
        if let Proj::FieldAt(index) = step {
            *index = self.moved_expr(index);
        }
        Ok(())
    }

    fn visit_lane_mask(
        &mut self,
        mask: &mut mojito_types::types::LaneMask,
    ) -> Result<(), Self::Error> {
        let Ok(()) = mask
            .try_map_exprs(&mut |expr| Ok::<_, std::convert::Infallible>(self.moved_expr(expr)))
            .map(|moved| *mask = moved);
        Ok(())
    }

    fn visit_param_arg(
        &mut self,
        arg: &mut MirParamArg,
        _: crate::mir::visit::ParamArgSite,
    ) -> Result<(), Self::Error> {
        if let Some(expr) = &mut arg.expr {
            *expr = self.moved_expr(expr);
        }
        Ok(())
    }

    fn visit_param_decls(
        &mut self,
        decls: &mut [ParamDecl],
        _: crate::mir::visit::ParamDeclSite,
    ) -> Result<(), Self::Error> {
        self.move_decls(decls);
        Ok(())
    }

    fn visit_constant(&mut self, constant: &mut Const) -> Result<(), Self::Error> {
        match constant {
            Const::Param(expr) => *expr = self.moved_expr(expr),
            Const::Value(value) => {
                if let Ok(moved) = mojito_types::types::replace_value_parameters(
                    &self.context,
                    value,
                    &self.bindings,
                ) {
                    *value = moved;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn visit_param_ref(
        &mut self,
        param: &mut mojito_types::param_expr::ParamRef,
    ) -> Result<(), Self::Error> {
        if let Some(own) = self.owned.get(&param.id) {
            *param = own.binder();
        }
        Ok(())
    }

    fn visit_constraint(&mut self, cond: &mut GenericConstraint) -> Result<(), Self::Error> {
        self.move_constraint(cond);
        Ok(())
    }

    fn visit_sequence(
        &mut self,
        source: &mut mojito_checked::checked::ComptimeSequence,
    ) -> Result<(), Self::Error> {
        let Ok(()) = source
            .try_map_exprs(&mut |expr| Ok::<_, std::convert::Infallible>(self.moved_expr(expr)))
            .map(|moved| *source = moved);
        Ok(())
    }
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
    /// The checked symbol.
    mangled: String,
    captures: Vec<NestedCapture>,
    callable_ty: Option<Ty>,
    /// The closure's function type ([`NestedInfo::contract`]).
    contract: Option<Ty>,
    /// The storage struct the check declared for its closure
    /// ([`CheckedDef::storage`]).
    ///
    /// [`CheckedDef::storage`]: mojito_checked::checked::CheckedDef::storage
    storage: Option<mojito_checked::checked::CheckedClosureStorage>,
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

/// The declaration at `site`, named as the check named it.
fn analyze_node(
    checked: &mojito_checked::checked::CheckedProgram,
    site: NestedDefSite,
) -> NestedNode {
    let NestedDefSite { node, loops } = site;
    let statement = checked
        .tables()
        .statement(node)
        .expect("a nested def names a node of its tables");
    let (declaration, def) = checked_nested_declaration(checked, node);
    let name = &def.name;
    let binding = declaration
        .binding
        .expect("checked nested function has no value binding");

    let captures = analyze_captures(def);
    let children = analyze_root_children(checked, &def.body, None);
    let callable_ty = checked
        .checked_type_at(&AnnotationSite::FunctionType {
            module: statement.module.clone(),
            declaration: statement.span,
            syntax: statement.syntax_id,
        })
        .cloned()
        .map(mir_nested_callable_ty);

    NestedNode {
        node,
        loop_indices: loops
            .iter()
            .filter_map(|id| checked.tables().comptime_loop_index(*id))
            .collect(),
        binding,
        source_name: name.clone(),
        mangled: def.symbol.clone(),
        captures,
        contract: def
            .storage
            .as_ref()
            .map(|storage| mir_nested_callable_ty(storage.contract.clone()))
            .or_else(|| callable_ty.clone()),
        callable_ty,
        storage: def.storage.clone(),
        defaults: def.params.iter().map(|param| param.default).collect(),
        children,
    }
}

/// Build the callable registry visible in one lexical function, whose
/// direct nested declarations are `children`. Direct declarations and inherited
/// callables are keyed by checked identity, so same-spelled bindings
/// coexist without shadow-removal heuristics. Every direct declaration
/// with a non-empty capture list, and every capturing lambda with or
/// without one, builds its storage struct ([`NestedInfo::storage`]), as
/// upstream's `constructClosure` and `resolveAnonymousClosure` emit a
/// closure instance for every one, and its body is the storage's method
/// ([`ClosureStorage::method`]); one without storage a sibling
/// captures keeps its closure in a slot ([`NestedInfo::slotted`]). A body
/// without storage takes its captures, if any, as leading reference
/// parameters (R1946). A declaration whose storage
/// the check recorded no uses for, one naming a binder no enclosing
/// declaration binds or one no lowering unit reaches, is an invariant
/// failure, recorded in `invariant_errors`.
fn scope_registry(
    children: &[NestedNode],
    inherited: &HashMap<mojito_types::origin::OwnerId, NestedInfo>,
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
        let storage = child
            .storage
            .as_ref()
            .and_then(|declared| closure_storage(child, declared, &registry));
        if child.storage.is_some() && storage.is_none() {
            invariant_errors.push(format!(
                "nested def `{}`: the check recorded no enclosing binders for its closure storage",
                child.source_name
            ));
        }
        let mangled = storage
            .as_ref()
            .and_then(|storage| storage.method.clone())
            .unwrap_or_else(|| child.mangled.clone());
        registry.insert(
            child.binding,
            NestedInfo {
                binding: child.binding,
                source_name: child.source_name.clone(),
                mangled,
                materialized_here: true,
                slotted: captured_by_sibling && storage.is_none(),
                storage,
                captures: child.captures.clone(),
                callable_ty: child.callable_ty.clone(),
                contract: child.contract.clone(),
            },
        );
    }
    registry
}

/// The storage struct of the capture-list `def` or capturing lambda
/// `child`, lowered from the declaration the check made (`declared`),
/// whose captured siblings `registry` registers: a reference field to a
/// captured sibling refers to its storage struct, and the struct is applied
/// to the enclosing binders the check collected
/// ([`CheckedClosureStorage::uses`]), as upstream's `emitClosureInstance`
/// applies it; [`own_storage_binders`] declares them as the storage's own
/// once its members are lowered. `None` for one the check recorded no uses
/// for.
///
/// [`CheckedClosureStorage::uses`]: mojito_checked::checked::CheckedClosureStorage::uses
fn closure_storage(
    child: &NestedNode,
    declared: &mojito_checked::checked::CheckedClosureStorage,
    registry: &HashMap<mojito_types::origin::OwnerId, NestedInfo>,
) -> Option<ClosureStorage> {
    let uses = declared.uses.as_ref()?;
    // The origin parameters follow the binders among the struct's
    // parameters, which number them.
    let origin_param = |origin: mojito_types::origin::OriginParamId| {
        mojito_types::origin::OriginParamId(uses.binders.len() as u32 + origin.0)
    };
    let fields: Vec<(String, Ty)> = declared
        .fields
        .iter()
        .zip(&child.captures)
        .map(|(field, capture)| {
            let ty = match field.reference {
                None => mir_nested_callable_ty(field.ty.clone()),
                Some(reference) => {
                    let referent = captured_storage(registry, capture)
                        .map_or_else(|| field.ty.clone(), |storage| storage.instance_ty.clone());
                    TyKind::Ref(mojito_types::origin::RefTy {
                        referent,
                        origin: mojito_types::origin::Origin::Param(origin_param(reference.origin)),
                        mutability: reference.mutability,
                    })
                    .into()
                }
            };
            (field.name.clone(), ty)
        })
        .collect();
    let binders = || uses.binders.iter().map(ParamDecl::own_argument);
    Some(ClosureStorage {
        ty: TyKind::Struct(
            declared.name.clone(),
            binders()
                .chain(declared.fields.iter().filter_map(|field| {
                    let origin = field.reference?.origin;
                    Some(TyArg::Origin(mojito_types::origin::Origin::Param(
                        origin_param(origin),
                    )))
                }))
                .collect(),
        )
        .into(),
        instance_ty: TyKind::Struct(
            declared.name.clone(),
            binders()
                .chain(declared.origin_arguments.iter().cloned().map(TyArg::Origin))
                .collect(),
        )
        .into(),
        fields,
        param_decls: uses.binders.clone(),
        parameters: uses.parameters.clone(),
        method: uses.method.then(|| {
            mojito_symbol::symbol::closure_method_name(&child.mangled, &child.source_name)
        }),
        declared: declared.clone(),
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

/// Push the declaration of the storage struct `storage`, if any, of the
/// closure `node`, whose captures its fields hold, and its synthesized
/// members: the fieldwise `__init__` the declaration statement calls, as
/// upstream's `ClosureEmitter` synthesizes it, reading a `var` capture,
/// taking a moved one, and storing a reference to a reference capture's
/// referent,
/// and, unless the storage is trivially register-passable, its fieldwise
/// `__deinit__`, `__moveinit__`, and (when every capture is copyable)
/// `__copyinit__`, as upstream's `liftClosure` synthesizes them; its
/// conformances follow (`Copyable` and `ImplicitlyCopyable` by
/// `allCopyable`, the `IsTrivially*` markers by triviality). A body that
/// is its method, declared under the closure's name, and owns a moved
/// capture writes its `self`: the struct lists it among the methods
/// writing `self`, so a call on the storage's place leaves the call's
/// `self` in it, as
/// upstream's call by address writes it in place. Returns the scope of
/// what it lowered, `None` for a closure without storage.
fn lower_closure_storage(
    storage: Option<&ClosureStorage>,
    node: &NestedNode,
    functions: &mut Vec<(String, MirFunction)>,
    declarations: &mut MirDeclarations,
) -> Option<StorageScope> {
    let storage = storage?;
    let captures = &node.captures;
    let method = &node.source_name;
    let from = LoweredFrom::at(functions, declarations);
    let copyable = storage.declared.copyable;
    let Some((name, _)) = storage.ty.kind().as_non_simd_struct() else {
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
    // Upstream's storage `__del__` is trivial exactly when the storage is
    // trivially register-passable (`ClosureEmitter`'s
    // `__del__is_trivial`), as are its copy and move.
    if !storage.declared.trivially_register_passable() {
        lower_synthesized_member(
            (name, &[]),
            &storage.fields,
            SynthesizedMember::Destroy,
            functions,
            declarations,
        );
    }
    // Upstream's `liftClosure` synthesizes the fieldwise move and, for a
    // copyable storage, copy; a trivially register-passable storage's are
    // the value itself.
    if !storage.declared.trivially_register_passable() {
        lower_synthesized_member(
            (name, &storage.param_decls),
            &storage.fields,
            SynthesizedMember::Move,
            functions,
            declarations,
        );
        if copyable {
            lower_synthesized_member(
                (name, &storage.param_decls),
                &storage.fields,
                SynthesizedMember::Copy,
                functions,
                declarations,
            );
        }
    }
    let writes_self = storage.method.is_some()
        && captures
            .iter()
            .any(|capture| capture.kind == mojito_ast::ast::CaptureKind::Move);
    declarations.structs.push(MirStructDeclaration {
        name: name.clone(),
        module: String::new(),
        fields: storage.fields.clone(),
        mut_self_methods: writes_self.then(|| method.clone()).into_iter().collect(),
        fieldwise_init: true,
        param_decls: storage.param_decls.clone(),
        explicit_destructors: HashMap::new(),
        explicit_destroy: None,
        conformances: [
            ("Copyable", copyable),
            ("ImplicitlyCopyable", copyable),
            ("Movable", true),
            ("Deinitable", true),
            (
                "IsTriviallyCopyable",
                storage.declared.trivially_register_passable(),
            ),
            (
                "IsTriviallyMovable",
                storage.declared.trivially_register_passable(),
            ),
            (
                "IsTriviallyDeinitable",
                storage.declared.trivially_register_passable(),
            ),
        ]
        .into_iter()
        .map(
            |(trait_name, holds)| mojito_checked::checked::StructConformance {
                trait_name: trait_name.to_string(),
                conditions: holds
                    .then_some(GenericConstraint::Bool(true))
                    .into_iter()
                    .collect(),
            },
        )
        .collect(),
        declared: ["Movable", "Deinitable"]
            .into_iter()
            .chain(
                copyable
                    .then_some(["Copyable", "ImplicitlyCopyable"])
                    .into_iter()
                    .flatten(),
            )
            .map(|trait_name| mojito_checked::checked::StructConformance {
                trait_name: trait_name.to_string(),
                conditions: vec![GenericConstraint::Bool(true)],
            })
            .chain(
                [
                    (
                        storage.declared.trivially_register_passable(),
                        "TrivialRegisterPassable",
                    ),
                    (storage.declared.register_passable(), "RegisterPassable"),
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
    Some(StorageScope {
        storage: name.clone(),
        parameters: storage.parameters.clone(),
        from,
        to: LoweredFrom::at(functions, declarations),
    })
}

/// Push upstream's `__call__` witness of the closure storage `storage`
/// (`emitStorageCallWitness`): an always-inline method forwarding every
/// argument to the storage's method, as `declarations` declares it, and
/// returning its result, so
/// the storage conforms to its closure trait and a call through a value of
/// it devirtualizes to the witness. A generic closure's witness takes the
/// method's parameters and forwards them as its own. A variadic or
/// reference-returning closure gets none: no closure trait spells its
/// signature.
fn lower_storage_call_witness(
    storage: &ClosureStorage,
    functions: &mut Vec<(String, MirFunction)>,
    declarations: &mut MirDeclarations,
) {
    let (Some((name, _)), Some(method)) = (
        storage.ty.kind().as_non_simd_struct(),
        storage
            .method
            .as_deref()
            .and_then(|method| declarations.function(method))
            .cloned(),
    ) else {
        return;
    };
    let method = &method;
    if method.variadic.is_some() || method.kw_variadic.is_some() || method.returns_reference {
        return;
    }
    let symbol = format!("{name}.__call__");
    let mut reg_types = HashMap::new();
    let mut register = |ty: &Ty| {
        let reg = Reg(reg_types.len() as u32);
        reg_types.insert(reg.0, ty.clone());
        reg
    };
    let root = |var: usize, ty: &Ty| MirPlace::root(var as VarId, Some(ty.clone()));
    let mut instrs = Vec::new();
    let recv = register(&storage.ty);
    instrs.push(MirInstr::LoadPlace {
        dest: recv,
        place: root(0, &storage.ty),
    });
    let mut args = Vec::new();
    let mut arg_places = Vec::new();
    for (index, (ty, convention)) in method
        .param_types
        .iter()
        .zip(&method.param_conventions)
        .enumerate()
    {
        let var = index + 1;
        let dest = register(ty);
        if is_owned(convention) {
            instrs.push(MirInstr::UseVar {
                dest,
                var: var as VarId,
                mode: UseMode::Move,
                constructor: None,
            });
            arg_places.push(None);
        } else {
            instrs.push(MirInstr::LoadPlace {
                dest,
                place: root(var, ty),
            });
            arg_places.push(Some(root(var, ty)));
        }
        args.push(dest);
    }
    let dest = register(&method.ret_ty);
    let error_ty = method.error_ty.clone().unwrap_or_else(Ty::error);
    let raises = method.raises.then(|| Raises {
        ty: error_ty.clone(),
        flag: register(&Ty::bool()),
        error: register(&error_ty),
    });
    // The witness's error slot follows `self` and the method's parameters.
    let error_slot = (method.param_types.len() + 1) as VarId;
    let writes_self = declarations
        .structure(name)
        .is_some_and(|declaration| declaration.mut_self_methods.contains(&method_name(method)));
    instrs.push(MirInstr::MethodCall {
        dest,
        recv,
        method: method_name(method),
        resolved: Some(method.lowered_name.clone()),
        raises: raises.clone(),
        reference_result: None,
        result_adapter: None,
        args,
        kwargs: Vec::new(),
        recv_place: (!storage.declared.register_passable()).then(|| Box::new(root(0, &storage.ty))),
        recv_writes: writes_self,
        receiver_convention: None,
        arg_places,
        kwarg_places: Vec::new(),
        capture_accesses: Vec::new(),
        param_arg_regs: Vec::new(),
        param_decls: method.param_decls.clone(),
        instantiated_args: method
            .param_decls
            .iter()
            .map(ParamDecl::own_argument)
            .collect(),
        spread: None,
        refined_bound: None,
    });
    if let Some(raises) = &raises {
        let region = |instrs, term| vec![MirBlock { instrs, term }];
        instrs.push(MirInstr::If {
            results: Vec::new(),
            cond: raises.flag,
            then: region(
                vec![MirInstr::DefVar {
                    var: error_slot,
                    src: raises.error,
                    binding_ty: Some(error_ty.clone()),
                    in_place: false,
                }],
                MirTerm::ErrorReturn,
            ),
            elifs: Vec::new(),
            else_args: Vec::new(),
            orelse: region(Vec::new(), MirTerm::Yield(Vec::new())),
        });
    }
    let returned = (method.ret_ty != TyKind::None).then_some(dest);
    if let Some(declaration) = declarations
        .structs
        .iter_mut()
        .find(|declaration| declaration.name == *name)
    {
        if writes_self {
            declaration.mut_self_methods.insert("__call__".to_string());
        }
        declaration.record_member("__call__", &symbol);
        declaration
            .witness_tables
            .extend(closure_trait_witness_table(method, &symbol));
    }
    let mut witness = method.clone();
    witness.lowered_name.clone_from(&symbol);
    witness.defaults = vec![None; method.param_types.len()];
    witness.required = vec![true; method.param_types.len()];
    witness.inline = mojito_checked::declaration::InlineLevel::Always;
    witness.provenance = mojito_checked::declaration::FunctionProvenance::Synthesized;
    witness.enclosing = None;
    declarations.functions.push(witness);
    let param_types: Vec<Ty> = std::iter::once(storage.ty.clone())
        .chain(method.param_types.iter().cloned())
        .collect();
    let n_params = param_types.len();
    let error_slot = method.raises.then_some(error_slot);
    let mut var_tys: HashMap<VarId, Ty> = param_types
        .iter()
        .enumerate()
        .map(|(var, ty)| (var as VarId, ty.clone()))
        .collect();
    var_tys.extend(error_slot.map(|slot| (slot, error_ty)));
    functions.push((
        symbol,
        MirFunction {
            blocks: vec![MirBlock {
                instrs,
                term: MirTerm::Return(returned),
            }],
            n_regs: reg_types.len() as u32,
            n_vars: n_params + usize::from(error_slot.is_some()),
            var_names: std::iter::once("self".to_string())
                .chain(method.param_names.iter().cloned())
                .chain(error_slot.map(|_| "$error".to_string()))
                .collect(),
            n_params,
            owned_params: std::iter::once(false)
                .chain(method.param_conventions.iter().map(is_owned))
                .collect(),
            deinit_params: vec![false; n_params],
            ref_params: std::iter::once(false)
                .chain(method.ref_params.iter().copied())
                .collect(),
            var_tys,
            param_types,
            reg_types,
            ret_ty: Some(method.ret_ty.clone()),
            raises: method.raises,
            error_ty: method.error_ty.clone(),
            error_slot,
            ..MirFunction::default()
        },
    ));
}

/// The storage's conformance to its closure trait, the function type of
/// the closure's `method`: its one requirement, `__call__`, answered by the
/// witness `symbol`, as a declared `def(...)` conformance records it.
fn closure_trait_witness_table(
    method: &MirFunctionDeclaration,
    symbol: &str,
) -> Option<mojito_checked::declaration::WitnessTable> {
    let arity = method.param_types.len();
    let closure_trait = Ty::new(TyKind::Func(Box::new(mojito_types::types::FuncTy {
        environment: mojito_types::origin::CallableEnvironment::Default,
        params: method.param_types.clone(),
        names: method.param_names.clone(),
        ret: method.ret_ty.clone(),
        required: vec![true; arity],
        variadic: None,
        kw_variadic: None,
        positional_only: None,
        keyword_only: None,
        raises: method.raises,
        error: method.error_ty.clone(),
        conventions: method.param_conventions.clone(),
        ref_params: vec![None; arity],
        ref_return: None,
        transfers: mojito_types::types::TransferSet::default(),
    })));
    let requirement = mojito_symbol::symbol::callable_contract_target(&closure_trait)?;
    Some(mojito_checked::declaration::WitnessTable {
        trait_name: closure_trait.to_string(),
        witnesses: vec![mojito_checked::declaration::Witness {
            requirement,
            member: symbol.to_string(),
        }],
    })
}

/// The source name of the closure method `method`, its symbol's last
/// segment.
fn method_name(method: &mojito_checked::declaration::FunctionDeclaration) -> String {
    mojito_symbol::symbol::split_method_symbol(&method.lowered_name)
        .map_or_else(|| method.lowered_name.clone(), |(_, name)| name.to_string())
}

/// The declaration a nested function is lowered inside: its lowered name,
/// the nested `def`s registered in its scope, the binders in scope there,
/// and its closures.
#[derive(Clone, Copy)]
struct Enclosing<'a> {
    name: &'a str,
    registry: &'a HashMap<mojito_types::origin::OwnerId, NestedInfo>,
    binders: &'a EnclosingBinders,
    closures: &'a ClosureLowering,
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
    let info = enclosing.registry.get(&node.binding);
    let storage = info.and_then(|info| info.storage.clone());
    // A body that is its storage's method is lowered under the method's
    // symbol, and the declarations nested in it are named under it, as
    // those of an ordinary method are.
    let method = storage
        .as_ref()
        .is_some_and(|storage| storage.method.is_some());
    let symbol = info.map_or_else(|| node.mangled.clone(), |info| info.mangled.clone());
    let enclosing_binders = &enclosing
        .binders
        .with_loops(&node.loop_indices)
        .with(&param_decls);
    let registry = scope_registry(&node.children, enclosing.registry, invariant_errors);
    let captures = &node.captures;
    let mangled = &symbol;
    let storage_scope = lower_closure_storage(storage.as_ref(), &node, out, declarations);
    let (capture_types, capture_writes) = capture_parameters(captures, enclosing.registry);
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
    // frame locals; a body without storage takes each capture as a
    // leading reference parameter (R1946).
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
    // A lifted invocation only borrows its storage or its captures; it
    // must neither destroy nor duplicate them on each call.
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
    // A capture without storage is `mut` (its final value is written back
    // to the enclosing variable); a storage is read.
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
    // A method's declaration lists only its own parameters, its receiver
    // apart.
    let (leading_names, leading_types, leading_writes) = if method {
        Default::default()
    } else {
        (leading_names, leading_types, leading_writes)
    };
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
    let capture_count = if method { 0 } else { leading };
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
        has_receiver: method,
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
        enclosing: (!method).then(|| enclosing.name.to_string()),
        region_binders: if method {
            Vec::new()
        } else {
            node.region_binders()
        },
        inline: def.inline,
        provenance: mojito_checked::declaration::FunctionProvenance::Source,
    });
    if let Some(storage) = storage.as_ref().filter(|_| method) {
        lower_storage_call_witness(storage, out, declarations);
    }
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
    let declared: HashMap<VarId, Ty> = ptys
        .iter()
        .enumerate()
        .map(|(slot, ty)| (slot as VarId, ty.clone()))
        .collect();
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
        enclosing_binders,
        &mut thunks,
        signature
            .effect
            .as_ref()
            .filter(|effect| effect.raises)
            .map(|effect| effect.error.clone().unwrap_or_else(Ty::error)),
    );
    thunks.request_applications(checked, &ncfg.body);
    thunks.inherit_callables(&registry);
    thunks.lower(checked, overloads, out, declarations, invariant_errors);
    nf.n_params = ptys.len();
    for (slot, ty) in ptys.iter().enumerate() {
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
                closures: enclosing.closures,
            },
        );
    }
    let method_end = method.then(|| LoweredFrom::at(out, declarations));
    enclosing
        .closures
        .push_storage_scope(storage_scope, method_end);
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

/// The type each of `captures` takes as a leading parameter of a body
/// without storage (R1946), whose captured siblings
/// `registry` registers, and whether a call of the body may write it. A
/// reference capture of a
/// reference binding (a loop's reference target) is a reference
/// parameter, typed by its referent as any is, and a capture of a sibling
/// closure with storage names that storage. An `imm` capture is an
/// immutable borrow at the call; a `mut` or `ref` one may write its place,
/// and so may a call of a captured sibling through storage it writes.
fn capture_parameters(
    captures: &[NestedCapture],
    registry: &HashMap<mojito_types::origin::OwnerId, NestedInfo>,
) -> (Vec<Ty>, Vec<bool>) {
    captures
        .iter()
        .map(|capture| match captured_storage(registry, capture) {
            Some(storage) => (storage.instance_ty.clone(), storage.declared.writes),
            None => (
                match capture.kind {
                    mojito_ast::ast::CaptureKind::Imm
                    | mojito_ast::ast::CaptureKind::Mut
                    | mojito_ast::ast::CaptureKind::Ref => capture_referent(capture),
                    _ => mir_nested_callable_ty(capture.ty.clone()),
                },
                capture.kind != mojito_ast::ast::CaptureKind::Imm,
            ),
        })
        .unzip()
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

/// Make each `return value` exit of an initializer store the value into
/// its `out self` receiver and return nothing, as `self = value; return`
/// does: upstream's `return` in an initializer stores into the result
/// slot. The exits are the function's own blocks' and those of the region
/// ops among them, never a `try` part's.
fn initialize_receiver_returns(function: &mut MirFunction) {
    let receiver = function
        .var_names
        .iter()
        .position(|name| name == "self")
        .expect("an initializer's receiver leads its variables") as VarId;
    initialize_receiver_exits(&mut function.blocks, receiver);
}

fn initialize_receiver_exits(blocks: &mut [MirBlock], receiver: VarId) {
    for block in blocks {
        for instruction in &mut block.instrs {
            if !matches!(instruction, MirInstr::Try { .. }) {
                for region in instruction_regions_mut(instruction) {
                    initialize_receiver_exits(region, receiver);
                }
            }
        }
        if let MirTerm::Return(Some(value)) = block.term {
            block.instrs.push(MirInstr::DefVar {
                var: receiver,
                src: value,
                binding_ty: None,
                in_place: false,
            });
            block.term = MirTerm::Return(None);
        }
    }
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

/// The declarations a function lowers whose body's statement nodes are
/// `ids`. A function returning a checked expression, one the check lifted
/// or an evaluated default, declares the lambdas of that expression alone
/// ([`CheckedTables::lifted_lambda_defs`]): those of the owner's
/// evaluated bindings its prologue reads are the owner's.
///
/// [`CheckedTables::lifted_lambda_defs`]: mojito_checked::checked::CheckedTables::lifted_lambda_defs
fn analyze_root_children(
    checked: &mojito_checked::checked::CheckedProgram,
    ids: &[CheckedStmtId],
    returned: Option<CheckedNodeId>,
) -> Vec<NestedNode> {
    let direct = match returned {
        None => checked.tables().nested_defs(ids),
        Some(returned) => checked
            .tables()
            .lifted_lambda_defs(returned)
            .into_iter()
            .map(|node| NestedDefSite {
                node,
                loops: Vec::new(),
            })
            .collect(),
    };
    direct
        .into_iter()
        .map(|site| analyze_node(checked, site))
        .collect()
}

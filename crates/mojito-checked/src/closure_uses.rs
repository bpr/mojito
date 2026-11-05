//! The enclosing binders each closure names, collected once its body is
//! checked.
//!
//! Upstream's `usesFromAbove` collects the values a closure uses from above
//! after its body is emitted: `emitClosureInstance` applies the closure's
//! storage struct to them, and `liftClosure` declares them as the struct's
//! own parameters.
//!
//! The pass runs as the check's last step, over the checked tables
//! ([`CheckedProgram::collect_closure_uses`]), and records its outcome on
//! each storage ([`CheckedClosureStorage::uses`]).

use crate::checked::{
    CheckedClosureStorage, CheckedClosureUses, CheckedDef, CheckedNodeId, CheckedOp,
    CheckedParamArg, CheckedProgram, CheckedStmt, CheckedStmtId, CheckedStmtOp, CheckedTables,
    ComptimeIteration, GenericSite, SemanticAdjustment, WalkResult, Walked,
};
use crate::unit::LoweringUnit;
use mojito_types::ct::CtValue;
use mojito_types::origin::OwnerId;
use mojito_types::param_expr::{ParamExpr, ParamId, ParamRef};
use mojito_types::types::{ParamDecl, Ty, TyArg, TyKind};
use std::collections::{HashMap, HashSet};

/// A declaration one lexical scope makes ([`CheckedTables::nested_defs`]):
/// its statement node, and the `comptime for` statements of that scope
/// enclosing it, outermost first.
pub struct NestedDefSite {
    pub node: CheckedStmtId,
    pub loops: Vec<CheckedStmtId>,
}

/// A `comptime for` index in scope: the binder the check recorded on the
/// loop's iterable, and the loop variable's checked binding.
#[derive(Clone)]
pub struct LoopIndex {
    pub declaration: ParamDecl,
    pub binding: Option<OwnerId>,
}

impl LoopIndex {
    /// The index of a loop the check recorded `iteration` on, its variable
    /// checked as `binding`.
    pub fn of(iteration: &ComptimeIteration, binding: Option<OwnerId>) -> Self {
        Self {
            declaration: iteration.index_declaration(),
            binding,
        }
    }
}

impl CheckedTables {
    /// The declarations of one lexical function scope, whose statement
    /// nodes are `ids`: those in its body, its control-flow blocks, and its
    /// kept compile-time regions, and the hidden `def` of each lambda among
    /// their statements' own expressions, without descending into another
    /// declaration's body.
    pub fn nested_defs(&self, ids: &[CheckedStmtId]) -> Vec<NestedDefSite> {
        let mut sites = Vec::new();
        self.find_nested_defs(ids, &mut Vec::new(), &mut sites);
        sites
    }

    /// The hidden `def` of each lambda in `expression`; a lambda inside a
    /// lambda is the inner scope's.
    pub fn expression_lambda_defs(&self, expression: CheckedNodeId) -> Vec<CheckedStmtId> {
        let mut defs = Vec::new();
        self.walk_expression(expression, &mut |walked| match walked {
            Walked::Expression(crate::checked::CheckedExpr {
                op: CheckedOp::Lambda { def },
                ..
            }) => {
                defs.push(*def);
                WalkResult::SkipBodies
            }
            _ => WalkResult::Advance,
        });
        defs
    }

    /// The index the `comptime for` statement `id` binds, as its body sees
    /// it; `None` for any other statement, or one the check recorded no
    /// iteration on.
    pub fn comptime_loop_index(&self, id: CheckedStmtId) -> Option<LoopIndex> {
        let node = self.statement(id)?;
        let CheckedStmtOp::ComptimeFor { iter, .. } = &node.op else {
            return None;
        };
        let iteration = self
            .expression(*iter)?
            .adjustments
            .iter()
            .find_map(|adjustment| match adjustment {
                SemanticAdjustment::ComptimeIteration(iteration) => Some(iteration),
                _ => None,
            })?;
        Some(LoopIndex::of(iteration, node.binding))
    }

    /// Record `uses` on the storage of the closure declared at `id`.
    pub(crate) fn set_closure_uses(&mut self, id: CheckedStmtId, uses: CheckedClosureUses) {
        if let Some(CheckedStmt {
            op: CheckedStmtOp::Def(def),
            ..
        }) = self.statements.get_mut(id.0 as usize)
            && let Some(storage) = &mut def.storage
        {
            storage.uses = Some(uses);
        }
    }

    fn find_nested_defs(
        &self,
        ids: &[CheckedStmtId],
        loops: &mut Vec<CheckedStmtId>,
        out: &mut Vec<NestedDefSite>,
    ) {
        for id in self.live_statements(ids) {
            let node = self
                .statement(*id)
                .expect("a body statement id names a node of its tables");
            // Lambda expressions anywhere in this statement's own expressions
            // (conditions, iterables, values, arguments) declare hidden nested
            // definitions in this scope.
            out.extend(
                statement_lambda_operands(&node.op)
                    .into_iter()
                    .flat_map(|expression| self.expression_lambda_defs(expression))
                    .map(|node| NestedDefSite {
                        node,
                        loops: loops.clone(),
                    }),
            );
            let bodies = match &node.op {
                CheckedStmtOp::Def(_) => {
                    out.push(NestedDefSite {
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
                self.find_nested_defs(ids, loops, out);
            }
            if region {
                loops.pop();
            }
        }
    }

    /// The `def` of the declaration at statement node `id`.
    fn def(&self, id: CheckedStmtId) -> Option<&CheckedDef> {
        match &self.statement(id)?.op {
            CheckedStmtOp::Def(def) => Some(def),
            _ => None,
        }
    }
}

impl CheckedProgram {
    /// Record on every closure storage of the program the enclosing binders
    /// its closure names ([`CheckedClosureStorage::uses`]), each lowering
    /// unit's closures over the binders in scope at its root: a function's
    /// or trait default's own, a method's or default wrapper's struct's and
    /// then its own, none at module scope. A closure's scope adds the
    /// indices of the `comptime for` loops around it, and the parameters of
    /// each declaration it is nested in.
    pub(crate) fn collect_closure_uses(&mut self) {
        let found = {
            let mut collector = Collector {
                tables: self.tables(),
                found: Vec::new(),
            };
            for unit in self.lowering_units() {
                self.collect_unit(unit, &mut collector);
            }
            let toplevel: Vec<CheckedStmtId> = self
                .tables()
                .roots()
                .iter()
                .copied()
                .filter(|root| {
                    self.tables().statement(*root).is_some_and(|statement| {
                        !matches!(
                            statement.op,
                            CheckedStmtOp::Def(_)
                                | CheckedStmtOp::Struct(_)
                                | CheckedStmtOp::Trait(_)
                                | CheckedStmtOp::Comptime { .. }
                        )
                    })
                })
                .collect();
            collector.scope(
                self.tables().nested_defs(&toplevel),
                &BinderScope::default(),
                &HashMap::new(),
            );
            collector.found
        };
        let tables = std::sync::Arc::get_mut(self.tables_mut())
            .expect("the check's tables are unshared while it records closure uses");
        for (id, uses) in found {
            tables.set_closure_uses(id, uses);
        }
    }

    fn collect_unit<'t>(&'t self, unit: LoweringUnit<'_>, collector: &mut Collector<'t>) {
        let tables = self.tables();
        let struct_parameters = |module: &Option<String>, declaration: &str| {
            self.generic_parameters_at(&GenericSite::Struct {
                module: module.clone(),
                declaration: declaration.to_string(),
            })
            .unwrap_or(&[])
        };
        match unit {
            LoweringUnit::Function { root } | LoweringUnit::TraitMember { root, .. } => {
                if let Some(def) = tables.def(root) {
                    collector.declaration(
                        &def.body,
                        def.params.iter().filter_map(|param| param.default),
                        &BinderScope::of(&def.signature.parameters),
                        &HashMap::new(),
                    );
                }
            }
            LoweringUnit::Method { root, member, .. } => {
                let Some(node) = tables.statement(root) else {
                    return;
                };
                let CheckedStmtOp::Struct(declaration) = &node.op else {
                    return;
                };
                let Some(method) = declaration.methods.get(member) else {
                    return;
                };
                collector.declaration(
                    &method.body,
                    method.params.iter().filter_map(|param| param.default),
                    &BinderScope::of(
                        struct_parameters(&node.module, &declaration.name)
                            .iter()
                            .chain(&method.signature.parameters),
                    ),
                    &HashMap::new(),
                );
            }
            LoweringUnit::Wrapper { wrapper, index, .. } => collector.declaration(
                tables.wrapper_roots(index),
                std::iter::empty(),
                &BinderScope::of(
                    struct_parameters(&wrapper.module, &wrapper.declaration)
                        .iter()
                        .chain(&wrapper.signature.parameters),
                ),
                &HashMap::new(),
            ),
            LoweringUnit::Constant { root } => collector.scope(
                tables.nested_defs(&[root]),
                &BinderScope::default(),
                &HashMap::new(),
            ),
            LoweringUnit::Destructor { .. }
            | LoweringUnit::FieldwiseConstructor { .. }
            | LoweringUnit::CopyConstructor { .. }
            | LoweringUnit::MoveConstructor { .. } => {}
        }
    }
}

/// The binders in scope at a declaration: the enclosing struct's and
/// functions', then its own.
#[derive(Clone, Default)]
struct BinderScope {
    /// The binders that are not callable, in scope order.
    declarations: Vec<ParamDecl>,
    /// The callable value binders ([`ParamDecl::is_callable_binder`]).
    callables: Vec<ParamDecl>,
    /// The enclosing `comptime for` loops' variables, each checked binding
    /// with the index binder it denotes.
    loop_bindings: Vec<(OwnerId, ParamRef)>,
}

impl BinderScope {
    fn of<'a>(declarations: impl IntoIterator<Item = &'a ParamDecl>) -> Self {
        Self::default().with(declarations)
    }

    /// These binders, then those of a declaration nested inside them.
    fn with<'a>(&self, declarations: impl IntoIterator<Item = &'a ParamDecl>) -> Self {
        let mut scope = self.clone();
        for declaration in declarations {
            if declaration.is_callable_binder() {
                scope.callables.push(declaration.clone());
            } else {
                scope.declarations.push(declaration.clone());
            }
        }
        scope
    }

    /// These binders, then the indices of the `comptime for` loops
    /// entered, outermost first.
    fn with_loops(&self, indices: &[LoopIndex]) -> Self {
        let mut scope = self.with(indices.iter().map(|index| &index.declaration));
        scope.loop_bindings.extend(
            indices
                .iter()
                .filter_map(|index| Some((index.binding?, index.declaration.binder()))),
        );
        scope
    }
}

/// The closure uses one [`CheckedProgram::collect_closure_uses`] found, by
/// the statement node of each closure's declaration.
struct Collector<'t> {
    tables: &'t CheckedTables,
    found: Vec<(CheckedStmtId, CheckedClosureUses)>,
}

impl Collector<'_> {
    /// Collect the uses of the closures of a declaration whose body is
    /// `body` and whose parameter defaults are `defaults`, declared where
    /// `scope` is in scope, with `registry` the storage type of each
    /// closure registered around it.
    fn declaration(
        &mut self,
        body: &[CheckedStmtId],
        defaults: impl Iterator<Item = CheckedNodeId>,
        scope: &BinderScope,
        registry: &HashMap<OwnerId, Ty>,
    ) {
        self.scope(self.tables.nested_defs(body), scope, registry);
        // A default's lambdas are a scope of their own, which registers
        // none of the body's closures.
        for default in defaults {
            let sites = self
                .tables
                .expression_lambda_defs(default)
                .into_iter()
                .map(|node| NestedDefSite {
                    node,
                    loops: Vec::new(),
                })
                .collect();
            self.scope(sites, scope, &HashMap::new());
        }
    }

    /// Collect the uses of the declarations `sites` one lexical scope
    /// makes, where `scope` is in scope and `inherited` registers the
    /// enclosing scopes' closures, then those nested in each: every
    /// declaration of the scope is registered before any body is entered,
    /// and a closure's storage refers only to those registered before it.
    fn scope(
        &mut self,
        sites: Vec<NestedDefSite>,
        scope: &BinderScope,
        inherited: &HashMap<OwnerId, Ty>,
    ) {
        let tables = self.tables;
        let mut registry = inherited.clone();
        let sites: Vec<(&CheckedDef, Vec<LoopIndex>, CheckedStmtId)> = sites
            .into_iter()
            .filter_map(|site| {
                let def = tables.def(site.node)?;
                let loops = site
                    .loops
                    .iter()
                    .filter_map(|id| tables.comptime_loop_index(*id))
                    .collect();
                Some((def, loops, site.node))
            })
            .collect();
        for (def, loops, node) in &sites {
            let Some(storage) = &def.storage else {
                continue;
            };
            let in_scope = scope.with(loops.iter().map(|index| &index.declaration));
            let Some(uses) = closure_uses(tables, def, storage, &in_scope, &registry) else {
                continue;
            };
            if let Some(binding) = tables
                .statement(*node)
                .and_then(|statement| statement.declaration)
                .and_then(|id| tables.declaration(id))
                .and_then(|declaration| declaration.binding)
            {
                registry.insert(
                    binding,
                    TyKind::Struct(
                        def.name.clone(),
                        uses.binders.iter().map(ParamDecl::own_argument).collect(),
                    )
                    .into(),
                );
            }
            self.found.push((*node, uses));
        }
        for (def, loops, _) in sites {
            self.declaration(
                &def.body,
                def.params.iter().filter_map(|param| param.default),
                &scope.with_loops(&loops).with(&def.signature.parameters),
                &registry,
            );
        }
    }
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

/// The uses of the closure `def`, whose storage the check declared as
/// `storage`, declared where `in_scope` is in scope, with `registry` the
/// storage type of each closure registered before it: the binders its
/// fields, its signature, and its body name ([`binder_uses`]), in scope
/// order, and whether its body is the storage's method. A reference field
/// to a registered closure names the binders that closure's storage is
/// applied to. `None` for one whose fields name a binder the scope does not
/// declare.
fn closure_uses(
    tables: &CheckedTables,
    def: &CheckedDef,
    storage: &CheckedClosureStorage,
    in_scope: &BinderScope,
    registry: &HashMap<OwnerId, Ty>,
) -> Option<CheckedClosureUses> {
    // A reference field names the binders its referent names.
    let contents: Vec<&Ty> = storage
        .fields
        .iter()
        .zip(&def.captures)
        .map(|(field, capture)| {
            let referent = match field.reference {
                Some(_) => registry.get(&capture.binding).unwrap_or(&field.ty),
                None => &field.ty,
            };
            match referent.kind() {
                TyKind::Ref(reference) if field.reference.is_none() => &reference.referent,
                _ => referent,
            }
        })
        .collect();
    let names_binder = |ty: &Ty, id: &ParamId| {
        mojito_types::types::mentions(
            ty,
            &|ty| matches!(ty.kind(), TyKind::Param { binder, .. } if binder.id == *id),
        )
    };
    let mut values = HashSet::new();
    for ty in &contents {
        mojito_types::types::referenced_parameters(ty, &mut values);
    }
    let uses = binder_uses(tables, def, in_scope);
    let named = |declaration: &ParamDecl| {
        uses.ids.contains(declaration.id())
            || uses
                .names
                .contains(declaration.name().trim_start_matches('*'))
    };
    let used = |declaration: &ParamDecl| {
        values.contains(declaration.name().trim_start_matches('*'))
            || contents.iter().any(|ty| names_binder(ty, declaration.id()))
            || named(declaration)
    };
    // A pack's implicit list binder (`Ts.values`) is a parameter wherever
    // its pack is: the pack's declared type and every element read name it.
    let binders: Vec<ParamDecl> = in_scope
        .declarations
        .iter()
        .filter(|declaration| {
            used(declaration)
                || declaration
                    .list_pack(&in_scope.declarations)
                    .is_some_and(used)
        })
        .cloned()
        .collect();
    let unbound = contents.iter().any(|ty| {
        mojito_types::types::mentions(ty, &|ty| {
            matches!(ty.kind(), TyKind::Param { binder, .. }
                if !binders.iter().any(|declaration| *declaration.id() == binder.id))
        })
    }) || values.iter().any(|name| {
        !binders
            .iter()
            .any(|declaration| declaration.name().trim_start_matches('*') == name)
    });
    (!unbound).then(|| CheckedClosureUses {
        method: !in_scope.callables.iter().any(named),
        binders,
    })
}

/// The binders a closure names ([`binder_uses`]): by identity, and by the
/// spelling of a value binder.
struct BinderUses {
    ids: HashSet<ParamId>,
    names: HashSet<String>,
}

/// The enclosing binders a closure's declaration `def` names, among those
/// `in_scope` declares: by identity, a type binder a type of its
/// signature or its body mentions, or the `comptime for` index a body
/// read binds; by name, a value binder such a type, a parameter
/// expression, or a body read spells. The body includes the declarations
/// nested in it, as upstream's `usesFromAbove` covers nested ops. The
/// closure's own parameters shadow the enclosing binders they spell.
fn binder_uses(tables: &CheckedTables, def: &CheckedDef, in_scope: &BinderScope) -> BinderUses {
    fn attribute(argument: &CheckedParamArg) -> Option<&TyArg> {
        match argument {
            CheckedParamArg::Attribute { value, .. } => value.as_ref(),
            CheckedParamArg::Named { value, .. } => attribute(value),
            CheckedParamArg::Value(_) => None,
        }
    }
    fn signature_types(signature: &crate::checked::CheckedSignature) -> Vec<Ty> {
        signature
            .params
            .iter()
            .flatten()
            .chain(&signature.result)
            .chain(
                signature
                    .effect
                    .as_ref()
                    .and_then(|effect| effect.error.as_ref()),
            )
            .cloned()
            .collect()
    }
    let mut types = signature_types(&def.signature);
    let mut exprs: Vec<ParamExpr> = Vec::new();
    let mut names = HashSet::new();
    let mut ids = HashSet::new();
    let mut reads = Vec::new();
    let mut declared: HashSet<OwnerId> =
        def.captures.iter().map(|capture| capture.binding).collect();
    let mut visit = |walked: Walked<'_>| {
        match walked {
            Walked::Statement(statement) => {
                declared.extend(statement.binding);
                types.extend(statement.binding_ty.clone());
                if let CheckedStmtOp::Def(nested) = &statement.op {
                    types.extend(signature_types(&nested.signature));
                }
            }
            Walked::Expression(expression) => {
                types.extend(
                    [&expression.ty, &expression.place_ty, &expression.binding_ty]
                        .into_iter()
                        .flatten()
                        .cloned(),
                );
                declared.extend(
                    expression
                        .comprehension_bindings
                        .iter()
                        .map(|binding| binding.owner),
                );
                match &expression.op {
                    CheckedOp::Identifier(name) | CheckedOp::Call { name, .. } => {
                        reads.push((name.clone(), expression.binding));
                    }
                    CheckedOp::TypeValue { ty: Some(ty), .. } => types.push(ty.clone()),
                    _ => {}
                }
                if let CheckedOp::Call { param_args, .. }
                | CheckedOp::Invoke { param_args, .. }
                | CheckedOp::TypeApply {
                    args: param_args, ..
                } = &expression.op
                {
                    for argument in param_args.iter().filter_map(attribute) {
                        match argument {
                            TyArg::Ty(ty) | TyArg::Val(CtValue::Type(ty)) => {
                                types.push(ty.clone());
                            }
                            TyArg::Val(CtValue::Expr(expr)) => exprs.push(expr.clone()),
                            TyArg::Val(_) | TyArg::Origin(_) => {}
                        }
                    }
                }
                for adjustment in &expression.adjustments {
                    if let SemanticAdjustment::ParamValue { value, .. } = adjustment {
                        exprs.push(value.clone());
                    }
                }
            }
        }
        WalkResult::Advance
    };
    tables.walk(&def.body, &mut visit);
    for default in def.params.iter().filter_map(|param| param.default) {
        tables.walk_expression(default, &mut visit);
    }
    // A read of a name the body does not declare is a read of an
    // enclosing binder of that name, or of the `comptime for` index its
    // binding is.
    for (name, binding) in reads {
        if binding.is_some_and(|binding| declared.contains(&binding)) {
            continue;
        }
        ids.extend(
            in_scope
                .loop_bindings
                .iter()
                .filter(|(loop_binding, _)| Some(*loop_binding) == binding)
                .map(|(_, binder)| binder.id.clone()),
        );
        names.insert(name);
    }
    for expr in &exprs {
        expr.referenced_parameters(&mut names);
        for reference in expr.free_parameters() {
            ids.insert(reference.id);
        }
    }
    // `mentions` does not descend into a reference: each referent is
    // walked in turn.
    let mut pending = types;
    let mentioned = std::cell::RefCell::new(ids);
    while let Some(ty) = pending.pop() {
        mojito_types::types::referenced_parameters(&ty, &mut names);
        let referents = std::cell::RefCell::new(Vec::new());
        mojito_types::types::mentions(&ty, &|ty| {
            match ty.kind() {
                TyKind::Param { binder, .. } => {
                    mentioned.borrow_mut().insert(binder.id.clone());
                }
                TyKind::Ref(reference) => referents.borrow_mut().push(reference.referent.clone()),
                _ => {}
            }
            false
        });
        pending.extend(referents.into_inner());
    }
    let ids = mentioned.into_inner();
    for own in def
        .signature
        .parameters
        .iter()
        .map(ParamDecl::name)
        .chain(def.params.iter().map(|param| param.name.as_str()))
    {
        names.remove(own.trim_start_matches('*'));
    }
    BinderUses { ids, names }
}

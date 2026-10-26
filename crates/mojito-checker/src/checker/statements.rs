//! Program, block, and statement checking: the top-level `check_program`
//! entry, block scoping, the `check_stmt` statement dispatcher, and parameter
//! type lowering. Extracted from `checker.rs`; see `docs/symbol-map.md`.

use super::comptime_validation::ComptimeTest;
use super::decorators::DecoratorSite;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_common::timing;
use mojito_types::types::TyKind;
use mojito_types::types::{FuncTy, GenericFuncTy, TransferSet};

impl Checker {
    pub fn check_program(&mut self, stmts: &[Stmt]) -> Result<(), TypeError> {
        let phase = timing::span("declarations.tuple_prepass");
        // A module integer constant folds once, ahead of every declaration
        // whose symbol spells it.
        self.fold_module_constants(stmts);
        // An importer's own statements add to its own sets, which a name
        // they miss falls through to the package's.
        std::sync::Arc::make_mut(&mut self.overload_sets).scan_more(stmts);
        self.default_wrappers = self.plan_default_wrappers(stmts)?;
        self.method_binder_owners =
            mojito_symbol::symbol::MethodBinderOwners::scan(stmts, &*self.overload_sets, &|name| {
                self.program_constants.get(name)
            });
        let applicable = super::comptime_validation::applicable_functions(
            stmts,
            &*self.overload_sets,
            &|name| self.program_constants.get(name),
        );
        self.applicable_functions.extend(applicable);
        for statement in stmts {
            if let StmtKind::Def {
                name,
                type_params,
                ret: Some(ret),
                ..
            } = &statement.kind
                && type_params.is_empty()
            {
                self.module_def_results
                    .entry(name.clone())
                    .or_insert_with(|| ret.clone());
            }
        }
        self.declared_structs
            .extend(stmts.iter().filter_map(|statement| match &statement.kind {
                StmtKind::Struct { name, .. } => Some(name.clone()),
                _ => None,
            }));
        self.nested_origins_read_only_functions
            .extend(stmts.iter().filter_map(|statement| match &statement.kind {
                StmtKind::Def {
                    name, decorators, ..
                } if mojito_ast::ast::is_nested_origins_read_only(decorators) => Some(name.clone()),
                _ => None,
            }));
        for statement in stmts {
            if let StmtKind::Comptime {
                name,
                type_params,
                value,
                ..
            } = &statement.kind
                && type_params.is_empty()
                && (super::lifts_comptime_value(value, self)
                    || names_read(value)
                        .iter()
                        .any(|read| self.applied_module_constants.contains(read)))
            {
                self.applied_module_constants.insert(name.clone());
            }
        }
        drop(phase);
        let phase = timing::span("declarations.shells");
        // Same-module declarations resolve order-independently: every
        // top-level struct registers its shell (name and parameters), then
        // every trait, then every struct's field/associated types, then every
        // struct's method signatures — all before the source-order walk below
        // checks conformance and method bodies. A struct field or method
        // signature may therefore reference a struct declared later in its
        // module (an iterator holding `ref[o] List[T]` above `List` itself).
        // Trait names predeclare before the shells: a struct parameter's
        // bound may name a user trait declared anywhere in the module
        // (`struct Wrap[T: Show]`); the trait's requirements register in
        // the trait pass below, before any body needs them.
        for statement in stmts {
            if let StmtKind::Trait { name, .. } = &statement.kind {
                self.predeclared_traits.insert(name.clone());
            }
        }
        // A shell whose parameter is typed by another struct's member
        // (`values: _MLIR.KGENParamListType[type]`) waits for that struct's
        // member types, and registers as soon as they resolve.
        let mut deferred_shells = Vec::new();
        let mut waiting_aliases = Vec::new();
        for (index, statement) in stmts.iter().enumerate() {
            // A type-parameter default may still spell the alias it names
            // (`H: Hasher = default_hasher`): registering module-level
            // aliases in source order beside the shells lets a shell resolve
            // an earlier alias and an alias body an earlier shell.
            if self.register_module_alias_when_declared(statement)? {
                waiting_aliases.push(index);
            }
            let Some(declaration) = struct_declaration(statement) else {
                continue;
            };
            match self.check_struct_shell(&declaration) {
                Ok(()) => {
                    self.predeclared_structs
                        .insert(declaration.name.to_string());
                }
                Err(_) if super::traits::names_member_type(declaration.type_params) => {
                    deferred_shells.push(index);
                }
                Err(error) => return Err(error),
            }
        }
        drop(phase);
        let phase = timing::span("declarations.traits");
        // A trait requirement's default folds against the module's constants,
        // which a trait may name wherever they are declared. A constant
        // whose initializer applies a callable is its application (decision
        // D3), whose identity a type keeps and a body's read evaluates.
        for statement in stmts {
            let StmtKind::Comptime {
                name,
                type_params,
                value,
                ..
            } = &statement.kind
            else {
                continue;
            };
            if !type_params.is_empty() {
                continue;
            }
            // The constant is bound ahead of its statement, so a body
            // declared before it reads it; the statement binds it again.
            let predeclared = if self.program_constants.own(name).is_some() {
                Some(Ty::int_literal())
            } else if let Some(applied) = self.applied_constant_expr(value) {
                let ty = applied.meta().as_value().cloned();
                self.comptime_applied.insert(name.clone(), applied);
                ty
            } else if super::traits::literal_default(value) {
                self.comptime_literals.insert(name.clone(), value.clone());
                self.infer(value).ok()
            } else if let Some(folded) = self
                .eval_associated_ct(value, &HashMap::new())
                .ok()
                .filter(CtValue::is_folded_parameter_argument)
            {
                self.module_values.insert(name.clone(), folded);
                self.infer(value).ok()
            } else {
                None
            };
            if let Some(ty) = predeclared
                && self.lookup(name).is_none()
            {
                self.declare_immutable(name, ty)?;
                self.predeclared_constants.insert(name.clone());
            }
        }
        // A trait whose signature reads a struct member (`_MLIR.
        // KGENParamListType[AnyType]`) or constructs a struct (`TypeList[...]()`)
        // waits for that struct's members or method signatures, and so does
        // every struct conforming to it: a conformer takes the trait's
        // `comptime` defaults.
        let mut deferred_traits = Vec::new();
        for (index, statement) in stmts.iter().enumerate() {
            let StmtKind::Trait {
                name,
                refines,
                methods,
                comptime_members,
            } = &statement.kind
            else {
                continue;
            };
            if refines
                .iter()
                .any(|parent| trait_is_deferred(stmts, &deferred_traits, parent))
            {
                deferred_traits.push(index);
                continue;
            }
            match self.check_trait(name, refines, methods, comptime_members) {
                Ok(()) => {
                    self.predeclared_traits.insert(name.clone());
                }
                Err(error)
                    if awaits_struct_declarations(&error)
                        || matches!(&error, TypeError::UnknownType(name)
                            if self.declared_structs.contains(name)) =>
                {
                    deferred_traits.push(index);
                }
                Err(error) => return Err(error),
            }
        }
        // Generic comptime aliases register before struct types and method
        // signatures: a field annotation or a synthesized conditional-method
        // availability (e.g. `copy()` gated on a `Copyable where MyPred[T]`
        // condition) may reference an alias regardless of declaration order.
        // The pre-pass keeps source order, so an alias body still sees only
        // earlier aliases.
        drop(phase);
        let phase = timing::span("declarations.aliases");
        for (index, statement) in stmts.iter().enumerate() {
            if self.register_module_alias_when_declared(statement)?
                && !waiting_aliases.contains(&index)
            {
                waiting_aliases.push(index);
            }
        }
        drop(phase);
        let phase = timing::span("declarations.types");
        let mut waiting_conformers = Vec::new();
        let mut constructing_structs = Vec::new();
        for (index, statement) in stmts.iter().enumerate() {
            let Some(declaration) = struct_declaration(statement) else {
                continue;
            };
            if deferred_shells.contains(&index) {
                deferred_shells.retain(|deferred| *deferred != index);
                self.check_struct_shell(&declaration)?;
                self.predeclared_structs
                    .insert(declaration.name.to_string());
            }
            if conforms_to_deferred(stmts, &deferred_traits, &declaration) {
                waiting_conformers.push(index);
                continue;
            }
            match self.check_struct_types(&declaration) {
                Ok(()) => {}
                Err(error) if self.awaits_construction(&error) => constructing_structs.push(index),
                Err(error) => return Err(error),
            }
            self.retry_deferred_shells(stmts, &mut deferred_shells);
        }
        // A waiting alias registers once the struct types have.
        for index in waiting_aliases {
            self.register_module_alias(&stmts[index])?;
        }
        drop(phase);
        let phase = timing::span("declarations.signatures");
        let parked_conformers = waiting_conformers.clone();
        self.retry_deferred_traits(
            stmts,
            &mut deferred_traits,
            &mut waiting_conformers,
            &mut constructing_structs,
        )?;
        for (index, statement) in stmts.iter().enumerate() {
            let Some(declaration) = struct_declaration(statement) else {
                continue;
            };
            if parked_conformers.contains(&index) || constructing_structs.contains(&index) {
                continue;
            }
            self.check_struct_method_signatures(&declaration)?;
            self.retry_deferred_traits(
                stmts,
                &mut deferred_traits,
                &mut waiting_conformers,
                &mut constructing_structs,
            )?;
        }
        // A struct whose member type constructs another of this program's
        // registers once the others' signatures have.
        while !constructing_structs.is_empty() {
            let index = constructing_structs.remove(0);
            let Some(declaration) = struct_declaration(&stmts[index]) else {
                continue;
            };
            self.check_struct_types(&declaration)?;
            self.check_struct_method_signatures(&declaration)?;
            self.retry_deferred_traits(
                stmts,
                &mut deferred_traits,
                &mut waiting_conformers,
                &mut constructing_structs,
            )?;
        }
        // A trait still waiting reports its own rejection.
        for index in deferred_traits {
            if let StmtKind::Trait {
                name,
                refines,
                methods,
                comptime_members,
            } = &stmts[index].kind
            {
                self.check_trait(name, refines, methods, comptime_members)?;
            }
        }
        drop(phase);
        let phase = timing::span("bodies");
        let result = self.check_module_block(stmts);
        drop(phase);
        result
    }

    /// Check the module's statements in source order. A constant whose
    /// initializer the check lifts (`comptime D = {"a": f(1)}`) is declared
    /// ahead of the first declaration after which its initializer types, so
    /// a body declared before the constant reads it, as at the pin. Every
    /// trait member is checked after the walk, and then every conformer's
    /// wrapper that calls one.
    fn check_module_block(&mut self, stmts: &[Stmt]) -> Result<(), TypeError> {
        let mut pending: Vec<usize> = (0..stmts.len())
            .filter(|&index| {
                matches!(&stmts[index].kind, StmtKind::Comptime { name, type_params, .. }
                    if type_params.is_empty()
                        && self.applied_module_constants.contains(name)
                        && self.lookup(name).is_none())
            })
            .collect();
        for (index, stmt) in stmts.iter().enumerate() {
            pending.retain(|&constant| constant > index);
            if matches!(stmt.kind, StmtKind::Def { .. } | StmtKind::Struct { .. }) {
                let mut waiting = Vec::with_capacity(pending.len());
                for constant in pending {
                    if !self.predeclare_lifted_constant(&stmts[constant])? {
                        waiting.push(constant);
                    }
                }
                pending = waiting;
            }
            // `ret = None` marks "not inside a function", so a top-level
            // `return` is rejected; `in_loop = false` likewise rejects a
            // top-level `break`.
            self.check_block(std::slice::from_ref(stmt), None, false)?;
        }
        self.check_trait_members(stmts)?;
        self.check_default_wrappers(stmts)
    }

    /// Bind the module constant `statement` declares ahead of it when the
    /// check lifts its initializer and the initializer types here: `true`
    /// once the constant is bound. An initializer naming what is not
    /// declared yet waits for a later declaration, and its own statement
    /// reports one that never is; any other rejection of the initializer is
    /// the constant's, reported here.
    fn predeclare_lifted_constant(&mut self, statement: &Stmt) -> Result<bool, TypeError> {
        let StmtKind::Comptime {
            name, ty, value, ..
        } = &statement.kind
        else {
            return Ok(false);
        };
        if self.lookup(name).is_some() {
            return Ok(true);
        }
        let _position = self.comptime_position();
        let expression = match self.binderless_comptime_expression(
            name,
            value,
            super::comptime_validation::ConstantScope::Module(ty.as_ref()),
        ) {
            Ok(Some(expression)) => expression,
            Ok(None) | Err(TypeError::UndefinedVariable(_)) => return Ok(false),
            Err(error) => return Err(error),
        };
        let declared = match ty {
            Some(annotation) => {
                self.resolve_storage_annotation(annotation, super::StorageStrictness::AllowBare)
            }
            None => self
                .infer(value)
                .and_then(|found| self.inferred_binding_ty(&found, name)),
        };
        let Ok(declared) = declared else {
            return Ok(false);
        };
        let Some(scope) = self.local_comptime_parameters.last_mut() else {
            return Ok(false);
        };
        scope.insert(name.clone(), (0, expression));
        if self.declare_immutable(name, declared).is_err() {
            return Ok(false);
        }
        self.predeclared_constants.insert(name.clone());
        Ok(true)
    }

    /// Whether `value` applies a member (`TypeList.of[...]`) of a struct
    /// this program declares that the struct has yet to register.
    fn applies_unregistered_member(&self, value: &Expr) -> bool {
        let (ExprKind::Index { object, .. } | ExprKind::MultiIndex { object, .. }) = &value.kind
        else {
            return false;
        };
        let ExprKind::Member { object, field } = &object.kind else {
            return false;
        };
        let ExprKind::Identifier(base) = &object.kind else {
            return false;
        };
        self.declared_structs.contains(base)
            && !self
                .structs
                .get(base)
                .is_some_and(|info| info.parameterized_associated.contains_key(field))
    }

    /// Whether `value` names a module constant whose initializer applies a
    /// callable (`comptime E = A` over `comptime A = f(1)`): a parameter
    /// alias of that constant, which takes its parameter expression, not a
    /// type alias.
    fn names_applied_module_constant(&self, value: &Expr) -> bool {
        matches!(&value.kind, ExprKind::Identifier(source)
            if self.applied_module_constants.contains(source)
                && !self.module_constant_shadowed(source))
    }

    /// Register a module-level `comptime` alias declaration — a generic
    /// alias, or a constant whose body is a type expression — ahead of the
    /// declarations that may name it. Any other statement, and an alias
    /// already registered, is left alone.
    /// Register each deferred struct shell the member types resolved so far
    /// let classify, dropping it from `deferred`.
    fn retry_deferred_shells(&mut self, stmts: &[Stmt], deferred: &mut Vec<usize>) {
        deferred.retain(|index| {
            let Some(declaration) = struct_declaration(&stmts[*index]) else {
                return false;
            };
            if self.check_struct_shell(&declaration).is_err() {
                return true;
            }
            self.predeclared_structs
                .insert(declaration.name.to_string());
            false
        });
    }

    /// Check every deferred trait whose struct declarations have resolved,
    /// then the conformers no deferred trait holds back any longer: their
    /// member types, then their method signatures, which may in turn
    /// resolve another deferred trait.
    fn retry_deferred_traits(
        &mut self,
        stmts: &[Stmt],
        deferred: &mut Vec<usize>,
        waiting: &mut Vec<usize>,
        constructing: &mut Vec<usize>,
    ) -> Result<(), TypeError> {
        loop {
            let mut resolved = false;
            let mut index = 0;
            while index < deferred.len() {
                let StmtKind::Trait {
                    name,
                    refines,
                    methods,
                    comptime_members,
                } = &stmts[deferred[index]].kind
                else {
                    deferred.remove(index);
                    continue;
                };
                let parent_waits = refines
                    .iter()
                    .any(|parent| trait_is_deferred(stmts, deferred, parent));
                if !parent_waits
                    && self
                        .check_trait(name, refines, methods, comptime_members)
                        .is_ok()
                {
                    self.predeclared_traits.insert(name.clone());
                    deferred.remove(index);
                    resolved = true;
                } else {
                    index += 1;
                }
            }
            if !resolved {
                return Ok(());
            }
            let ready: Vec<usize> = waiting
                .iter()
                .copied()
                .filter(|&conformer| {
                    struct_declaration(&stmts[conformer]).is_some_and(|declaration| {
                        !conforms_to_deferred(stmts, deferred, &declaration)
                    })
                })
                .collect();
            waiting.retain(|conformer| !ready.contains(conformer));
            for conformer in ready {
                let Some(declaration) = struct_declaration(&stmts[conformer]) else {
                    continue;
                };
                match self.check_struct_types(&declaration) {
                    Ok(()) => self.check_struct_method_signatures(&declaration)?,
                    Err(error) if self.awaits_construction(&error) => constructing.push(conformer),
                    Err(error) => return Err(error),
                }
            }
        }
    }

    /// Whether `error` is a member type's construction of a struct of this
    /// program (`TypeList[...]()[id]` over the bundled library linked from
    /// source) before that struct's signatures register: the declaration
    /// waits for them.
    fn awaits_construction(&self, error: &TypeError) -> bool {
        matches!(error, TypeError::BadCall { func, reason }
            if reason == "no '__init__' overload takes no arguments"
                && self.declared_structs.contains(func))
    }

    /// [`Self::register_module_alias`], or `true` when the alias waits for
    /// the pass after the struct types: it applies a member a struct of
    /// this program has yet to register (`TypeList.of[...]` over the
    /// bundled library linked from source), or it fails while some struct
    /// shell of this program has yet to register (one waiting on another
    /// struct's member types), whose declarations the body may need.
    fn register_module_alias_when_declared(&mut self, statement: &Stmt) -> Result<bool, TypeError> {
        if let StmtKind::Comptime { value, .. } = &statement.kind
            && self.applies_unregistered_member(value)
        {
            return Ok(true);
        }
        match self.register_module_alias(statement) {
            Err(_)
                if self
                    .declared_structs
                    .iter()
                    .any(|name| !self.predeclared_structs.contains(name)) =>
            {
                Ok(true)
            }
            registered => registered.map(|()| false),
        }
    }

    fn register_module_alias(&mut self, statement: &Stmt) -> Result<(), TypeError> {
        let StmtKind::Comptime {
            name,
            type_params,
            ty,
            where_clauses,
            value,
        } = &statement.kind
        else {
            return Ok(());
        };
        if self.comptime_aliases.contains_key(name) {
            return Ok(());
        }
        // A single-argument type application parses as a subscript
        // (`AHasher[SIMD[DType.uint64, 4](0)]`): the subscript shape
        // registers only over a declared struct.
        let subscript_application = matches!(&value.kind, ExprKind::Index { object, .. }
                if matches!(&object.kind, ExprKind::Identifier(base)
                    if self.declared_structs.contains(base)));
        // A struct member's application (`TypeList.of[Trait=AnyType, Int]`)
        // names a type the same way.
        let member_application = matches!(&value.kind,
            ExprKind::Index { object, .. } | ExprKind::MultiIndex { object, .. }
                if matches!(&object.kind, ExprKind::Member { object, .. }
                    if matches!(&object.kind, ExprKind::Identifier(base)
                        if self.declared_structs.contains(base)
                            || self.structs.contains_key(base))));
        if type_params.is_empty() && self.names_applied_module_constant(value) {
            return Ok(());
        }
        if type_params.is_empty()
            && !subscript_application
            && !member_application
            && !matches!(
                value.kind,
                ExprKind::Identifier(_) | ExprKind::TypeApply { .. } | ExprKind::TypeValue(_)
            )
        {
            return Ok(());
        }
        self.check_generic_comptime_alias(name, type_params, ty.as_ref(), where_clauses, value)
    }

    /// Check the statements of a block in the current scope. `ret` is the
    /// enclosing function's declared return type (or `None` at module level);
    /// `in_loop` is true inside a `while`/`for` body (gating `break`/`continue`).
    /// A `comptime assert` makes its condition known to the statements after
    /// it, to the end of this block alone.
    pub(super) fn check_block(
        &mut self,
        stmts: &[Stmt],
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<(), TypeError> {
        let mut asserted = 0;
        let result = stmts.iter().try_for_each(|stmt| {
            if let StmtKind::ComptimeAssert { cond, message } = &stmt.kind {
                let constraint = self.check_comptime_assert(cond, message.as_ref())?;
                self.assume_condition(cond, constraint.as_ref())?;
                asserted += 1;
                return Ok(());
            }
            self.check_stmt(stmt, ret, in_loop)?;
            if let StmtKind::Comptime { name, .. } = &stmt.kind
                && let Some(owner) = self.lookup_owner(name)
            {
                self.comptime_binding_owners.insert(owner);
            }
            Ok(())
        });
        let mut assumed = self.assumed_conformances.borrow_mut();
        let depth = assumed.len() - asserted;
        assumed.truncate(depth);
        result
    }

    /// Check a block in a fresh nested scope (the body of an `if`/`elif`/`else`
    /// or loop). The new scope is popped before returning.
    pub(super) fn check_scoped_block(
        &mut self,
        stmts: &[Stmt],
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<(), TypeError> {
        self.push_scope();
        let result = self.check_block(stmts, ret, in_loop);
        self.pop_scope();
        result
    }

    /// Check an `if`/`elif`/`else` chain, or under source validation a
    /// `comptime if` chain (`comptime`), every arm in its own scope.
    ///
    /// Definite initialization follows only reachable exits when a condition
    /// is a `Bool` literal: every source branch is still checked, but
    /// `if True: x = ...` establishes a function-scoped implicit binding just
    /// as an unconditional assignment does. Any other condition retains both
    /// the taken and the fallthrough possibilities — for a `comptime if` the
    /// selection is the elaborator's, so no arm is assumed here.
    pub(super) fn check_conditional(
        &mut self,
        branches: &[(mojito_ast::ast::Condition, Vec<Stmt>)],
        orelse: Option<&[Stmt]>,
        ret: Option<&Ty>,
        in_loop: bool,
        comptime: bool,
    ) -> Result<(), TypeError> {
        let before = self.uninitialized.borrow().clone();
        let mut exits = Vec::new();
        let mut fallthrough_reachable = true;
        for (cond, body) in branches {
            (*self.uninitialized.borrow_mut()).clone_from(&before);
            if let mojito_ast::ast::Condition::Case { pattern, value } = cond
                && !comptime
            {
                self.check_case_condition(pattern, value, body, ret, in_loop)?;
                if fallthrough_reachable {
                    exits.push(self.uninitialized.borrow().clone());
                }
                continue;
            }
            let cond = Self::boolean_condition(cond, "if")?;
            if comptime {
                let constraint = self.check_comptime_condition(cond, ComptimeTest::If)?;
                // What the condition proves holds in this arm alone.
                self.assume_condition(cond, constraint.as_ref())?;
                let checked = self.check_scoped_block(body, ret, in_loop);
                self.assumed_conformances.borrow_mut().pop();
                checked?;
            } else {
                self.register_named_bindings(cond)?;
                self.expect_bool(cond, "if condition")?;
                self.check_scoped_block(body, ret, in_loop)?;
            }
            let condition = match &cond.kind {
                ExprKind::Bool(value) => Some(*value),
                _ => None,
            };
            if fallthrough_reachable && condition != Some(false) {
                exits.push(self.uninitialized.borrow().clone());
            }
            if condition == Some(true) {
                fallthrough_reachable = false;
            }
        }
        if let Some(body) = orelse {
            *self.uninitialized.borrow_mut() = before;
            self.check_scoped_block(body, ret, in_loop)?;
            if fallthrough_reachable {
                exits.push(self.uninitialized.borrow().clone());
            }
        } else if fallthrough_reachable {
            exits.push(before);
        }
        *self.uninitialized.borrow_mut() = exits.into_iter().flatten().collect::<HashSet<_>>();
        Ok(())
    }

    /// The boolean expression of a condition. A `case` condition reaching
    /// here is a `comptime if case`, rejected until R1248 lands.
    pub(super) fn boolean_condition<'c>(
        condition: &'c mojito_ast::ast::Condition,
        statement: &str,
    ) -> Result<&'c Expr, TypeError> {
        match condition {
            mojito_ast::ast::Condition::Expr(value) => Ok(value),
            mojito_ast::ast::Condition::Case { .. } => Err(TypeError::Unsupported(format!(
                "'{statement} case' pattern conditions"
            ))),
        }
    }

    /// Whether `ty` is a type parameter one of whose bounds requires the
    /// in-place dunder of `op`, which `x OP= y` then dispatches through the
    /// bound as the call `x.__iadd__(y)` would. A parameter whose bounds do
    /// not keeps the builtin operator path.
    pub(super) fn bound_inplace_operator(&self, ty: &Ty, op: InfixOp) -> bool {
        let TyKind::Param { bounds, .. } = ty.kind() else {
            return false;
        };
        op.inplace_dunder()
            .is_some_and(|dunder| !self.lookup_trait_methods(bounds, dunder, 1).is_empty())
    }

    /// Select the in-place dunder (`__iadd__`, …) for `receiver OP= value` on a
    /// user-defined `operand_ty`, or on a type parameter whose bound requires
    /// it, returning its complete checked method-call
    /// contract. The contract (and the call's effects/conversions) is keyed at
    /// `span`. A missing in-place dunder is a hard error: Mojo dispatches
    /// augmented assignment to the dedicated method and does not fall back to the
    /// ordinary binary operator. `__iadd__` returns `None`; a value-returning
    /// method is rejected.
    pub(super) fn select_inplace_operator(
        &self,
        span: &SourceSpan,
        receiver: &Expr,
        op: InfixOp,
        value: &Expr,
        operand_ty: &Ty,
    ) -> Result<mojito_checked::checked::CheckedCallContract, TypeError> {
        let missing = || TypeError::MissingInPlaceOperator {
            op: infix_symbol(op).to_string(),
            ty: operand_ty.to_string(),
        };
        let dunder = op.inplace_dunder().ok_or_else(missing)?;
        let has_method = match operand_ty.kind() {
            TyKind::Struct(name, _) => self
                .structs
                .get(name)
                .is_some_and(|info| info.methods.contains_key(dunder)),
            _ => self.bound_inplace_operator(operand_ty, op),
        };
        if !has_method {
            return Err(missing());
        }
        let args = std::slice::from_ref(value);
        let ret = self.infer_method_call(
            span,
            receiver,
            dunder,
            MethodCallArguments::ordinary(args, &[]),
        )?;
        if ret != TyKind::None {
            return Err(TypeError::TypeMismatch {
                expected: "None".to_string(),
                found: ret.to_string(),
                context: format!("in-place operator '{dunder}'"),
            });
        }
        let contract = self
            .fact(span, |facts| facts.selected_call.as_deref())
            .ok_or_else(|| {
                TypeError::InvariantViolation(
                    "in-place operator lost its selected call contract".to_string(),
                )
            })?;
        // The receiver is a place, not a computed call. Keep the selected
        // contract solely in the `AugmentedInPlace` record so lowering treats the
        // receiver as a place (write-back through `recv_place`) rather than as a
        // reference-returning call result keyed at this span.
        self.clear_fact(span, |facts| facts.selected_call.take().map(|fact| *fact));
        self.clear_fact(span, |facts| facts.overload_target.take());
        self.clear_fact(span, |facts| facts.generic_instantiation.take());
        Ok(contract)
    }

    /// Select the in-place dunder for a nominal-subscript element
    /// (`c[i] += v` → `element.__iadd__(v)`) or a reference-returning call's
    /// referent (`p.name_ref() += s`). The receiver is a fresh mutable
    /// temporary typed as the element, declared in a throwaway scope, so
    /// selection reuses `select_inplace_operator` without re-inferring the
    /// subscript `place` — which would disturb the getter/setter adjustment state
    /// this branch carefully snapshots. The RHS conversion is recorded on the real
    /// `value` span and survives the scope pop; lowering applies the returned
    /// contract to the element through a reference getter's handle, or to a
    /// value getter's result before the setter receives it.
    ///
    /// The temporary is no binding of the body: what the selection recorded
    /// at it, which is no expression, is dropped with it, as are the
    /// generations it invalidates below it, and its identity is released when
    /// its scope pops, so the body's own bindings are numbered as if it had
    /// never been declared.
    pub(super) fn select_subscript_inplace_operator(
        &mut self,
        place: &Expr,
        op: InfixOp,
        value: &Expr,
        operand_ty: &Ty,
    ) -> Result<mojito_checked::checked::CheckedCallContract, TypeError> {
        let temporary = self.peek_owner();
        self.push_scope();
        let selection = self
            .declare("$inplace_elem", operand_ty.clone())
            .and_then(|()| {
                let mut receiver = Expr::new(
                    ExprKind::Identifier("$inplace_elem".to_string()),
                    place.span,
                );
                receiver.source.clone_from(&place.source);
                let mut contract = self.select_inplace_operator(
                    &receiver.source_span(),
                    &receiver,
                    op,
                    value,
                    operand_ty,
                )?;
                self.remove_occurrence_facts(&receiver.source_span());
                let rooted = |invalidation: &mojito_checked::checked::InteriorInvalidation| {
                    invalidation.base.root == temporary
                };
                contract.boundary.invalidations.retain(|fact| !rooted(fact));
                for argument in &mut contract.boundary.arguments {
                    argument.invalidations.retain(|fact| !rooted(fact));
                }
                Ok(contract)
            });
        self.pop_scope();
        self.transferred_origins.borrow_mut().remove(&temporary);
        if self.next_owner.get() == temporary.0 + 1 {
            self.next_owner.set(temporary.0);
        }
        let contract = selection?;
        // The temporary hides the element place from the selected call's
        // within-call exclusivity check, so the real place is the receiver
        // here: `xs[1] += xs[0]` lends `xs` mutably and immutably at once.
        let copied = contract.arguments.first().is_some_and(|argument| {
            self.argument_is_independent_copy(argument.convention, value, &argument.parameter_ty)
        });
        crate::checker::places::check_receiver_aliasing(
            place,
            contract.receiver_convention,
            &[ArgSlot::Positional(0)],
            &[copied],
            std::slice::from_ref(value),
            &[],
        )?;
        Ok(contract)
    }

    /// Record the in-place dunder `call` selected for the place at `span`,
    /// keeping the `ReferenceResult` a module `def` call place recorded at the
    /// same span, which no selected-call contract carries.
    pub(super) fn record_augmented_in_place(
        &self,
        span: SourceSpan,
        call: mojito_checked::checked::CheckedCallContract,
    ) {
        let mut record = self.record_at(span);
        let place_reference = match record.adjustment.as_deref() {
            Some(mojito_checked::checked::SemanticAdjustment::ReferenceResult { reference }) => {
                Some(reference.clone())
            }
            _ => None,
        };
        record.adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::AugmentedInPlace {
                call: Box::new(call),
                place_reference,
            },
        ));
    }

    /// The store-outward escape rule shared by ordinary place assignment and
    /// unpack-into-place: a store whose destination roots at outliving
    /// storage (a parameter or `self`) must not smuggle a frame-local loan
    /// outward — the symmetric twin of the Return escape check. Rebinding a
    /// `ref` destination counts even when the value type carries no loan:
    /// the destination handle itself becomes the loan. An accepted store
    /// records its transfer effect for call-site replay.
    #[allow(clippy::ref_option, reason = "TODO: take Option<&T>")]
    pub(super) fn check_outward_store(
        &self,
        place: &Expr,
        value: &Expr,
        found: &Ty,
        storage: &Option<Ty>,
    ) -> Result<(), TypeError> {
        // Outward storage: the frame's outliving owners (`self`, `mut`/`ref`
        // parameters, capture-reachable extensions), or ANY enclosing-frame
        // binding a nested def reaches through captures — a store into either
        // outlives this frame, so a frame-locally rooted loan dangles.
        let outward = self
            .aggregate_escape_contexts
            .last()
            .is_some_and(|(_, allowed)| {
                place_root_name(place)
                    .and_then(|root| self.lookup_owner(root))
                    .is_some_and(|owner| {
                        allowed.contains(&owner) || self.owner_in_enclosing_scope(owner)
                    })
            });
        if outward
            && (self.type_carries_loans(found)
                || matches!(storage.as_ref().map(Ty::kind), Some(TyKind::Ref(_))))
        {
            let mut origins = self.aggregate_origins(value);
            // A rebinding store's loan roots at the right-hand place
            // itself, which a plain value expression does not surface
            // as an aggregate origin.
            if matches!(storage.as_ref().map(Ty::kind), Some(TyKind::Ref(_)))
                && let Ok(rhs_place) = self.origin_place(value)
            {
                origins.push(mojito_types::origin::Origin::Place(rhs_place));
            }
            if origins
                .iter()
                .any(|origin| self.aggregate_origin_escapes(origin))
            {
                return Err(TypeError::StoredReferenceEscapesOrigin);
            }
            // Accepted: every origin is caller-visible. Record the
            // transfer so later call sites install the caller-side
            // loan this store implies.
            self.record_transfer_effect(place, &origins, storage, None);
        } else if outward
            && mojito_types::types::is_symbolic(found)
            && self.type_may_carry_loans(found)
        {
            // A symbolic value carries no loan here, but an instance at a
            // loan-carrying argument records this store's effect: keep it
            // latent for the instances derived from this body.
            let origins = self.aggregate_origins(value);
            if origins
                .iter()
                .any(|origin| self.aggregate_origin_escapes(origin))
            {
                if let Some(frame) = self.transfer_frames.borrow_mut().last_mut() {
                    frame.latent_escapes = true;
                }
                self.record_transfer_effect(place, &[], storage, Some(found));
            } else {
                self.record_transfer_effect(place, &origins, storage, Some(found));
            }
        }
        Ok(())
    }

    /// The synthetic element reads of unpacking a value of tuple type `vt`:
    /// each element's checked type and, for the nominal `Tuple`, its index
    /// accessor read through the reference the value yields (`source`).
    pub(super) fn tuple_unpack_plan(
        &self,
        vt: &Ty,
        source: Option<&mojito_types::origin::RefTy>,
    ) -> Result<Vec<mojito_checked::checked::CheckedTupleUnpackElement>, TypeError> {
        let elements = tuple_elements(vt).ok_or_else(|| {
            TypeError::InvariantViolation(format!("unpacking a value of non-tuple type {vt}"))
        })?;
        let mut plan = elements
            .into_iter()
            .cloned()
            .map(|ty| mojito_checked::checked::CheckedTupleUnpackElement {
                ty,
                accessor: None,
                param_decls: Vec::new(),
                temporary: None,
                reference: None,
                carries_loans: false,
                elements: Vec::new(),
            })
            .collect::<Vec<_>>();
        // The nominal `Tuple` reads each element through its
        // compile-time-index accessor, `__getitem_param__[index]`, on the
        // reference the unpacked place (or materialized temporary) yields.
        if let Some((name, accessor)) = self.tuple_index_accessor(vt)
            && let Some(source) = source
        {
            for element in &mut plan {
                element.accessor = Some(format!("{name}.__getitem_param__"));
                element.param_decls.clone_from(&accessor.decls);
                let reference = match element.ty.kind() {
                    // A reference-valued element forwards its own handle.
                    TyKind::Ref(reference) => reference.clone(),
                    _ => mojito_types::origin::RefTy {
                        referent: element.ty.clone(),
                        origin: source.origin.clone(),
                        mutability: source.mutability,
                    },
                };
                element.ty = reference.referent.clone();
                element.reference = Some(reference);
            }
        }
        for element in &mut plan {
            element.carries_loans = self.type_carries_loans(&element.ty);
        }
        Ok(plan)
    }

    /// The nominal `Tuple` `vt` names, with its compile-time-index accessor
    /// (`__getitem_param__[index: Int](ref self)`).
    pub(super) fn tuple_index_accessor<'s>(
        &'s self,
        vt: &'s Ty,
    ) -> Option<(&'s str, &'s MethodSig)> {
        let TyKind::Struct(name, _) = vt.kind() else {
            return None;
        };
        tuple_elements(vt)?;
        let [accessor] = self
            .structs
            .get(name)?
            .methods
            .get("__getitem_param__")?
            .as_slice()
        else {
            return None;
        };
        Some((name.as_str(), accessor))
    }

    #[allow(
        clippy::cognitive_complexity,
        clippy::too_many_lines,
        reason = "TODO: split this pass"
    )]
    pub(super) fn check_stmt(
        &mut self,
        stmt: &Stmt,
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<(), TypeError> {
        match &stmt.kind {
            StmtKind::RefDecl { name, value } => {
                let reference = match self.infer(value)?.into_kind() {
                    TyKind::Ref(reference) => reference,
                    _ => {
                        // Ordinary expression inference reads through a
                        // reference-returning method to its referent.  The
                        // method call nevertheless left an exact checked
                        // `ReferenceResult` adjustment, which a `ref` binding
                        // must retain instead of trying to reinterpret the call
                        // expression as a syntactic place. The materializing
                        // variant consumes that checked result, remains the
                        // shared fallback for an ordinary place binding, and
                        // accepts an owned temporary (`ref x = make_list()`)
                        // by minting it a hidden owned slot, upstream's
                        // temporary-lifetime rule.
                        self.materialized_reference_actual(value)?
                    }
                };
                // An erased `rebind` retypes the referent the binding views.
                let mut reference = reference;
                reference.referent = self.apply_rebind_target(value, reference.referent.clone())?;
                let mutable = reference.mutability == mojito_types::origin::Mutability::Mutable;
                // A reference to an ordinary projection below a named owned
                // interior (for example `dict[key].field`) carries the full
                // projected generation, even though only the nested index
                // expression originally introduced the interior fact. Retain
                // that canonical origin on the binding expression so MIR can
                // establish the projected generation rather than degrading it
                // to an untracked ordinary place loan.
                if let mojito_types::origin::Origin::Place(origin) = &reference.origin
                    && origin.path.iter().any(|segment| {
                        matches!(segment, mojito_types::origin::OriginSeg::Interior(_))
                    })
                {
                    self.record_at(value.source_span()).interior_reference = Some(origin.clone());
                }
                self.record_at(value.source_span()).reference_value_use = Some(mutable);
                let binding_ty = Ty::new(TyKind::Ref(reference));
                self.record_at(stmt.source_span()).binding_ty = Some(binding_ty.clone());
                self.declare_with_mutability(name, binding_ty, mutable)?;
                self.record_statement_binding(stmt, name);
                Ok(())
            }
            StmtKind::VarDecl { name, ty, value } => {
                // Current Mojo reads a bare `def(...)` local annotation as
                // trait-typed storage, like fields and collection elements;
                // a stored callable spells `def(...) thin`. The explicit
                // `capturing[...]` local annotation stays accepted as
                // Mojito's spelling for closure-storing locals (upstream
                // stores those un-annotated) — a recorded divergence.
                if let Some(mojito_ast::ast::Type::Func {
                    thin: false,
                    capturing: None,
                    ..
                }) = ty
                {
                    return Err(TypeError::Unsupported(format!(
                        "a 'def(...)' type names a trait in the annotation of variable \
                         '{name}' in current Mojo, not a storable callable value; spell \
                         a stored callable 'def(...) thin' or a stored closure \
                         'def(...) capturing[...]'"
                    )));
                }
                if matches!(value.kind, ExprKind::Uninitialized) {
                    let Some(annotation) = ty else {
                        return Err(TypeError::Unsupported(
                            "an uninitialized variable requires a type annotation".to_string(),
                        ));
                    };
                    let declared = self
                        .resolve_storage_annotation(annotation, super::StorageStrictness::Full)?;
                    self.record_at(stmt.source_span()).binding_ty = Some(declared.clone());
                    self.declare(name, declared)?;
                    self.record_statement_binding(stmt, name);
                    if let Some(owner) = self.lookup_owner(name) {
                        self.uninitialized.borrow_mut().insert(owner);
                    }
                    return Ok(());
                }
                // A type has no runtime value: it binds through `comptime`.
                if let ExprKind::Identifier(source) = &value.kind
                    && self.lookup(source).is_none()
                    && (self.structs.contains_key(source)
                        || self.traits.contains_key(source)
                        || self.comptime_aliases.contains_key(source))
                {
                    return Err(TypeError::DynamicTypeValue(name.clone()));
                }
                // Nor has a trait (`var x = Tr` over `Tr: type_of(AnyType)`).
                if let ExprKind::Identifier(source) = &value.kind
                    && self
                        .lookup(source)
                        .is_some_and(|ty| matches!(ty.kind(), TyKind::AnyTrait(_)))
                {
                    return Err(TypeError::DynamicTypeValue(name.clone()));
                }
                self.register_named_bindings(value)?;
                let (contextual, origin_demands) = match ty.as_ref() {
                    Some(annotation) => {
                        let (contextual, demands) = self.resolve_storage_annotation_with_origins(
                            annotation,
                            super::StorageStrictness::AllowBare,
                        )?;
                        (Some(contextual), demands)
                    }
                    None => (None, Vec::new()),
                };
                let found = match contextual.as_ref() {
                    Some(expected) => self.infer_with_expected(value, expected, true)?,
                    None => self.infer(value)?,
                };
                // A borrowing view conversion (`var s: Span[Int, _] = xs`)
                // consumes the conversion temporary, not the source place.
                let conversion_borrows_source =
                    self.storage_conversion_borrows_source(&found, contextual.as_ref());
                if !conversion_borrows_source {
                    self.check_consuming(value, &found, &format!("variable '{name}'"))?;
                }
                let declared = match ty {
                    // Annotated: the value must coerce to the annotation.
                    Some(anno) => {
                        let expected = contextual.unwrap_or(self.resolve_storage_annotation(
                            anno,
                            super::StorageStrictness::AllowBare,
                        )?);
                        // Current Mojo does not infer a generic function's
                        // specialization from local-storage context; only an
                        // explicit specialization (`f[...]`) materializes a
                        // generic function as a stored callable value.
                        if matches!(found.kind(), TyKind::GenericFunc(_))
                            && matches!(expected.kind(), TyKind::Func(_))
                            && matches!(value.kind, ExprKind::Identifier(_))
                        {
                            return Err(TypeError::Unsupported(format!(
                                "cannot materialize a generic function as the '{expected}' \
                                 value of variable '{name}': current Mojo does not infer a \
                                 specialization from a local annotation; specialize \
                                 explicitly or bind a non-generic function"
                            )));
                        }
                        // A `SIMD[dt, _]` width is solved from the initializer
                        // before the conversion, so the value converts to the
                        // vector the binding declares.
                        let expected = match expected.kind() {
                            TyKind::Simd { width, .. } if width.is_inferred() => {
                                Self::bind_unbound_tails(&expected, &found)
                            }
                            _ => expected,
                        };
                        if contains_infer(&expected) {
                            if contains_infer(&found) {
                                return Err(TypeError::CannotInferTypeParam {
                                    name: expected.to_string(),
                                    param: "_".to_string(),
                                });
                            }
                            found.clone()
                        } else {
                            if !self.record_implicit_conversion(value, &found, &expected)? {
                                if let Some(mismatch) =
                                    self.struct_origin_mismatch(&found, &expected)
                                {
                                    return Err(mismatch);
                                }
                                return Err(TypeError::TypeMismatch {
                                    expected: expected.to_string(),
                                    found: found.to_string(),
                                    context: format!("variable '{name}'"),
                                });
                            }
                            // An origin slot the annotation left to inference
                            // takes the initializer's origin, as upstream.
                            Self::bind_unbound_tails(&expected, &found)
                        }
                    }
                    // Inferred `var x = e`: declare the value's materialized type.
                    None if matches!(value.kind, ExprKind::TypeApply { .. })
                        && self.has_fact(&value.source_span(), |facts| {
                            facts.overload_target.is_some()
                        }) =>
                    {
                        // Explicitly specializing an Origin parameter produces
                        // a first-class, non-capturing function value whose
                        // checked `TyKind::Func` contains the bound origin. Plain
                        // inferred function/closure values remain non-escaping.
                        found.clone()
                    }
                    // An un-annotated string binding materializes the nominal
                    // `String`, like the other literal defaults; the wrap
                    // falls back to the compile-time string on seams without
                    // the linked stdlib struct.
                    None if found == TyKind::StringLiteral => {
                        self.nominal_string_wrap(value.source_span())?
                    }
                    None => self.inferred_binding_ty(&found, name)?,
                };
                if ty.is_none() {
                    self.record_literal_materializations(value, &found, &declared)?;
                }
                self.record_at(value.source_span()).binding_ty = Some(declared.clone());
                if self.is_deinitable(&declared) {
                    self.explicit_destroy_deletability
                        .borrow_mut()
                        .bindings
                        .insert(value.source_span());
                } else if matches!(declared.kind(), TyKind::Param { .. }) {
                    self.explicit_destroy_deletability
                        .borrow_mut()
                        .linear_bindings
                        .insert(value.source_span());
                }
                let (aggregate_origins, aggregate_field_origins) =
                    if !matches!(declared.kind(), TyKind::Ref(_))
                        && self.type_may_carry_loans(&declared)
                    {
                        (
                            self.aggregate_origins(value),
                            self.aggregate_field_origins(value),
                        )
                    } else {
                        (Vec::new(), HashMap::new())
                    };
                if !origin_demands.is_empty() {
                    // The annotation's explicit origins are demands on the
                    // initializer: the value must borrow what they name.
                    let mut actual_origins = aggregate_origins.clone();
                    if conversion_borrows_source && let Ok(place) = self.origin_place(value) {
                        actual_origins.push(mojito_types::origin::Origin::Place(place));
                    }
                    self.check_storage_origin_demands(
                        &format!("variable '{name}'"),
                        &origin_demands,
                        &actual_origins,
                    )?;
                }
                self.declare(name, declared)?;
                self.record_statement_binding(stmt, name);
                // Recorded unconditionally: a re-declaration without origin
                // arguments clears a stale demand.
                self.set_storage_origin_demands(name, origin_demands);
                self.set_aggregate_origins(name, aggregate_origins);
                self.set_aggregate_field_origins(name, aggregate_field_origins);
                let immutable_binders = self.construction_immutable_binders(value);
                self.set_immutable_origin_binders(name, immutable_binders);
                Ok(())
            }

            StmtKind::Assign { name, value } => {
                self.register_named_bindings(value)?;
                // `_ = e` discards the value: evaluate it so any move/effect still
                // registers, but introduce no binding and require none.
                if name == "_" {
                    let found = self.infer(value)?;
                    // A discarded reference result is not read.
                    self.discarded_reference_results
                        .borrow_mut()
                        .insert(value.source_span());
                    self.check_consuming(value, &found, "discard assignment")?;
                    self.record_discarded_transfer(value);
                    return Ok(());
                }
                self.check_capture_access(name, true)?;
                // Mojo treats a bare assignment in a function as a local
                // introduction unless that name is already local to this
                // function. Its initializer may still read an outer binding.
                let target = if let Some(&base) = self.function_bases.last() {
                    self.scopes[base..]
                        .iter()
                        .rev()
                        .find_map(|s| s.get(name))
                        .cloned()
                        .or_else(|| {
                            // A mutable captured variable is updated by reference;
                            // an immutable capture (notably `comptime`) is instead
                            // shadowed by a new function-local binding.
                            let mutable = self.mutable_scopes[..base]
                                .iter()
                                .rev()
                                .find_map(|s| s.get(name))
                                .copied()
                                .or_else(|| {
                                    self.installed_binding(name)
                                        .map(|(_, binding)| binding.mutable)
                                })
                                .unwrap_or(false);
                            if mutable {
                                self.scopes[..base]
                                    .iter()
                                    .rev()
                                    .find_map(|s| s.get(name))
                                    .or_else(|| {
                                        self.installed_binding(name).map(|(_, binding)| &binding.ty)
                                    })
                                    .cloned()
                            } else {
                                None
                            }
                        })
                } else {
                    self.lookup(name).cloned()
                };
                let target = self.rebind_assignment_target(stmt, name, target)?;
                self.record_assignment_rebind(stmt, value);
                // A re-assigned binding's type is the expected context, so a
                // collection display materializes against it (`values = [40]`
                // stays `List[Int]` for a List-typed variable).
                let found = match &target {
                    Some(target) => {
                        let expected = match target.kind() {
                            TyKind::Ref(reference) => reference.referent.clone(),
                            _ => target.clone(),
                        };
                        self.infer_with_expected(value, &expected, true)?
                    }
                    None => self.infer(value)?,
                };
                let assigned = target.as_ref().map(|target| match target.kind() {
                    TyKind::Ref(reference) => reference.referent.clone(),
                    _ => target.clone(),
                });
                let conversion_borrows_source =
                    self.storage_conversion_borrows_source(&found, assigned.as_ref());
                if !conversion_borrows_source {
                    self.check_consuming(value, &found, &format!("assignment to '{name}'"))?;
                }
                match target {
                    // Re-assignment: the value must keep the variable's type.
                    Some(target) => {
                        if !self.is_binding_mutable(name) {
                            return Err(TypeError::ImmutableBinding(name.clone()));
                        }
                        // Judged before the old value's invalidation, so the
                        // diagnostic names the aliasing call, not a stale use.
                        let destination = match target.kind() {
                            TyKind::Ref(reference) => match &reference.origin {
                                mojito_types::origin::Origin::Place(place) => Some(place.clone()),
                                _ => None,
                            },
                            _ => self.lookup_owner(name).map(|root| {
                                mojito_types::origin::OriginPlace {
                                    root,
                                    path: Vec::new(),
                                }
                            }),
                        };
                        if let Some(destination) = destination {
                            self.check_result_aliases_destination(&destination, value)?;
                        }
                        match target.kind() {
                            // Assignment to a reference binding writes through
                            // the handle; it does not rebind the handle.  Use
                            // the reference's checked origin so replacing a
                            // whole container also expires references into its
                            // owned interior regions.  The handle itself stays
                            // valid across that write.
                            TyKind::Ref(reference) => {
                                if let mojito_types::origin::Origin::Place(base) = &reference.origin
                                {
                                    self.record_origin_invalidation(
                                        value.source_span(),
                                        base.clone(),
                                        self.lookup_owner(name),
                                    );
                                }
                            }
                            _ => {
                                if let Some(owner) = self.lookup_owner(name) {
                                    self.record_owner_invalidation(
                                        value.source_span(),
                                        owner,
                                        Vec::new(),
                                    );
                                }
                            }
                        }
                        let target_is_reference = matches!(target.kind(), TyKind::Ref(_));
                        if let TyKind::Ref(reference) = target.kind() {
                            self.check_overwritten_referent(name, &reference.referent)?;
                        }
                        let target = match target.kind() {
                            TyKind::Ref(reference) => reference.referent.clone(),
                            _ => target,
                        };
                        // Assigning a closure, or an aggregate storing one,
                        // could move it to an outer binding; a thin function
                        // value captures nothing. A named generic or
                        // overloaded callable is not a first-class value here
                        // whatever its environment, as at `return`.
                        if matches!(found.kind(), TyKind::GenericFunc(_) | TyKind::Overload(_))
                            || holds_closure(&found)
                        {
                            return Err(TypeError::ClosureEscape);
                        }
                        // Recorded before the origins are read, as at the
                        // declaration: a view conversion's source place is an
                        // origin of the assigned value.
                        if !self.record_implicit_conversion(value, &found, &target)? {
                            if let Some(mismatch) = self.struct_origin_mismatch(&found, &target) {
                                return Err(mismatch);
                            }
                            return Err(TypeError::TypeMismatch {
                                expected: target.to_string(),
                                found: found.to_string(),
                                context: format!("assignment to '{name}'"),
                            });
                        }
                        let (aggregate_origins, aggregate_field_origins) =
                            if !target_is_reference && self.type_may_carry_loans(&target) {
                                (
                                    self.aggregate_origins(value),
                                    self.aggregate_field_origins(value),
                                )
                            } else {
                                (Vec::new(), HashMap::new())
                            };
                        // The annotation's explicit origins remain demands on
                        // every later assignment, exactly as on the initializer.
                        let origin_demands = self.lookup_storage_origin_demands(name);
                        if !origin_demands.is_empty() {
                            let mut actual_origins = aggregate_origins.clone();
                            if conversion_borrows_source && let Ok(place) = self.origin_place(value)
                            {
                                actual_origins.push(mojito_types::origin::Origin::Place(place));
                            }
                            self.check_storage_origin_demands(
                                &format!("assignment to '{name}'"),
                                &origin_demands,
                                &actual_origins,
                            )?;
                        }
                        if let Some(owner) = self.lookup_owner(name) {
                            self.uninitialized.borrow_mut().remove(&owner);
                        }
                        self.record_statement_binding(stmt, name);
                        self.set_aggregate_origins(name, aggregate_origins);
                        self.set_aggregate_field_origins(name, aggregate_field_origins);
                        let immutable_binders = self.construction_immutable_binders(value);
                        self.set_immutable_origin_binders(name, immutable_binders);
                        Ok(())
                    }
                    // Mojito requires `var` to introduce a new binding; a bare
                    // assignment to a name not in scope is an error.
                    None => Err(TypeError::AssignToUndeclared(name.clone())),
                }
            }

            StmtKind::AugAssign { place, op, value } => {
                if let Some(root) = place_root_name(place) {
                    self.check_capture_access(root, true)
                        .map_err(|error| match error {
                            TypeError::ImmutableBinding(name) => {
                                TypeError::ImmutableInPlaceDestination(name)
                            }
                            other => other,
                        })?;
                }
                let nominal_subscript = match &place.kind {
                    ExprKind::Index { object, index }
                        if self.type_keyed_projection(object, index) =>
                    {
                        false
                    }
                    ExprKind::Index { object, .. }
                    | ExprKind::Slice { object, .. }
                    | ExprKind::MultiIndex { object, .. } => {
                        matches!((self.infer(object)?).kind(), TyKind::Struct(name, _) if self.structs.contains_key(name))
                    }
                    _ => false,
                };
                // `target OP= value` means `target = target OP value`: the place
                // must be writable, and the result of the operator must keep the
                // place's type. A nominal subscript is two selected calls, not a
                // raw writable projection: infer its getter first, then select
                // the setter against the computed operator result.
                let shared_argument_sources = if nominal_subscript {
                    match &place.kind {
                        ExprKind::Index { index, .. } => vec![index.source_span()],
                        ExprKind::MultiIndex { args, .. } => {
                            args.iter()
                                .filter_map(|argument| match argument {
                                    SubscriptArg::Index(index)
                                    | SubscriptArg::Keyword { value: index, .. } => {
                                        Some(index.source_span())
                                    }
                                    SubscriptArg::Slice { .. }
                                    | SubscriptArg::KeywordSlice { .. } => None,
                                })
                                .collect()
                        }
                        ExprKind::Slice { .. } => Vec::new(),
                        _ => unreachable!("nominal subscript classification"),
                    }
                } else {
                    Vec::new()
                };
                let shared_adjustments = self.snapshot_value_adjustments(&shared_argument_sources);
                let in_place_destination = |error| match error {
                    TypeError::ImmutableBinding(name) => {
                        TypeError::ImmutableInPlaceDestination(name)
                    }
                    other => other,
                };
                let target = if nominal_subscript {
                    self.check_pointer_derefs(place)
                        .map_err(in_place_destination)?;
                    self.infer(place)?
                } else {
                    self.check_place(place).map_err(in_place_destination)?
                };
                let getter = if nominal_subscript {
                    let contract = self
                        .fact(&place.source_span(), |facts| facts.selected_call.as_deref())
                        .ok_or_else(|| {
                            TypeError::InvariantViolation(
                                "augmented nominal subscript lost its getter contract".to_string(),
                            )
                        })?;
                    // Provably immutable rejects; parametric-mut is judged per
                    // instantiation at the enclosing method's call sites.
                    if let Some(reference) = &contract.reference_result
                        && reference.mutability == mojito_types::origin::Mutability::Immutable
                    {
                        return Err(TypeError::ImmutableBinding(
                            "immutable reference returned by '__getitem__'".to_string(),
                        ));
                    }
                    Some(contract)
                } else {
                    None
                };
                if !nominal_subscript {
                    self.record_place_write_invalidation(place.source_span(), place);
                }
                if let Some(TyKind::Ref(reference)) =
                    self.place_storage_ty(place).as_ref().map(Ty::kind)
                    && reference.mutability != mojito_types::origin::Mutability::Mutable
                {
                    return Err(TypeError::ImmutableBinding(
                        "immutable reference field".to_string(),
                    ));
                }
                // A user-defined value dispatches augmented assignment to its
                // dedicated in-place dunder (`x += y` → `x.__iadd__(y)`), which
                // mutates `mut self`; Mojo does not fall back to the binary
                // operator. Native scalars keep the builtin operator path below.
                let inplace_operand = matches!(target.kind(), TyKind::Struct(name, _) if self.structs.contains_key(name))
                    || self.bound_inplace_operator(&target, *op);
                if !nominal_subscript && inplace_operand {
                    // A reference-returning call's facts share the place's
                    // span, so its dunder is selected on a temporary receiver.
                    let contract = if matches!(
                        place.kind,
                        ExprKind::Call { .. } | ExprKind::MethodCall { .. }
                    ) {
                        self.select_subscript_inplace_operator(place, *op, value, &target)?
                    } else {
                        self.select_inplace_operator(
                            &place.source_span(),
                            place,
                            *op,
                            value,
                            &target,
                        )?
                    };
                    self.record_augmented_in_place(place.source_span(), contract);
                    return Ok(());
                }
                // A user-struct subscript element dispatches augmented assignment
                // to its in-place dunder, exactly like a variable or field target;
                // a native element keeps the binary operator + setter path. The
                // mutated element keeps its own type, so `result` is the element
                // type and the value-getter's `__setitem__` selection binds it.
                let (result, inplace_contract) = if nominal_subscript && inplace_operand {
                    let contract =
                        self.select_subscript_inplace_operator(place, *op, value, &target)?;
                    (target.clone(), Some(contract))
                } else {
                    let result = self.infer_infix(None, *op, place, value)?;
                    if !coerces(&result, &target) {
                        return Err(TypeError::TypeMismatch {
                            expected: target.to_string(),
                            found: result.to_string(),
                            context: "augmented assignment".to_string(),
                        });
                    }
                    (result, None)
                };
                if nominal_subscript {
                    let site = place.source_span();
                    let getter = getter.expect("nominal getter was captured above");
                    if getter.reference_result.is_some() {
                        // A value-returning augmented subscript reaches the
                        // setter checker below, which also records descriptor
                        // shape. A mutable-reference getter has no setter, so a
                        // plain index must retain that metadata here for MIR.
                        if matches!(place.kind, ExprKind::Index { .. }) {
                            self.record_at(site.clone())
                                .subscript_descriptor
                                .get_or_insert_with(|| (vec![None], false));
                        }
                        // The store writes the element through the handle, so
                        // it stales every view lent below that element.
                        self.record_place_write_invalidation(site.clone(), place);
                        self.record_at(site.clone()).ty = Some(target.clone());
                        self.record_at(site).adjustment = Some(Box::new(
                            mojito_checked::checked::SemanticAdjustment::AugmentedSubscript(
                                Box::new(mojito_checked::checked::CheckedAugmentedSubscript {
                                    getter,
                                    setter: None,
                                    inplace: inplace_contract,
                                    operand_ty: target,
                                    result_ty: result,
                                }),
                            ),
                        ));
                        return Ok(());
                    }
                    // The getter and setter share syntax, not parameter
                    // adaptation or call effects. Freeze the getter above, then
                    // return those source-keyed compatibility tables to their
                    // pre-getter state before selecting the setter.
                    self.restore_value_adjustments(&shared_adjustments);
                    self.remove_call_boundary_invalidations(&site, &getter.boundary);

                    let mut computed = Expr::new(ExprKind::None, mojito_common::token::DUMMY_SPAN);
                    computed.source.clone_from(&place.source);
                    let value_source = computed.source_span();
                    self.record_at(value_source.clone()).ty = Some(result.clone());
                    self.check_nominal_subscript_assignment(place, &computed)?
                        .ok_or_else(|| {
                            TypeError::InvariantViolation(
                                "augmented nominal subscript lost its setter selection".to_string(),
                            )
                        })?;
                    let mut setter = self
                        .fact(&site, |facts| facts.selected_call.as_deref())
                        .ok_or_else(|| {
                            TypeError::InvariantViolation(
                                "augmented nominal subscript lost its setter contract".to_string(),
                            )
                        })?;
                    self.clear_fact(&value_source, |facts| facts.implicit_conversion.take());
                    self.clear_fact(&value_source, |facts| facts.conversion_source_borrow.take());
                    self.clear_fact(&value_source, |facts| facts.ty.take());
                    if matches!(
                        self.node_facts
                            .borrow()
                            .get(&value_source)
                            .and_then(|facts| facts.adjustment.as_deref()),
                        Some(mojito_checked::checked::SemanticAdjustment::MaterializeLiteral(_))
                    ) {
                        self.clear_fact(&value_source, |facts| facts.adjustment.take());
                    }
                    // The computed result is no expression: lowering binds
                    // its register to the setter directly, so the value is
                    // keyed at the store's own site and nothing is recorded
                    // for it as a temporary argument.
                    self.clear_flag(&value_source, |facts| &mut facts.read_temporary_argument);
                    self.unconsumed_temporaries
                        .borrow_mut()
                        .remove(&value_source);
                    for argument in &mut setter.boundary.arguments {
                        if argument.value_source == value_source {
                            argument.value_source.clone_from(&site);
                        }
                    }
                    let before_write = self
                        .fact(&site, |facts| facts.interior_invalidations.as_ref())
                        .unwrap_or_default();
                    self.record_place_write_invalidation(site.clone(), place);
                    if let Some(after_write) = self
                        .node_facts
                        .borrow()
                        .get(&site)
                        .and_then(|facts| facts.interior_invalidations.as_ref())
                    {
                        let existing = setter.boundary.invalidations.clone();
                        let additional = after_write
                            .iter()
                            .filter(|fact| !before_write.contains(fact))
                            .filter(|fact| !existing.contains(fact))
                            .cloned()
                            .collect::<Vec<_>>();
                        setter.boundary.invalidations.extend(additional);
                    }
                    self.restore_value_adjustments(&shared_adjustments);
                    self.remove_call_boundary_invalidations(&site, &setter.boundary);
                    self.record_at(site.clone()).selected_call = Some(Box::new(setter.clone()));
                    self.record_at(site.clone()).ty = Some(target.clone());
                    self.record_at(site).adjustment = Some(Box::new(
                        mojito_checked::checked::SemanticAdjustment::AugmentedSubscript(Box::new(
                            mojito_checked::checked::CheckedAugmentedSubscript {
                                getter,
                                setter: Some(setter),
                                inplace: inplace_contract,
                                operand_ty: target,
                                result_ty: result,
                            },
                        )),
                    ));
                }
                Ok(())
            }

            // Destructuring `a, b = t`, `(var a, (ref b, _)) = pair`,
            // `var [a, b] = values`: the value's pieces go to the target's
            // leaves (`destructure.rs`).
            StmtKind::Destructure { target, value } => {
                self.check_destructure(stmt, target, value, ret, in_loop)
            }

            // The context-manager protocol is a checked desugar (`with_stmt.rs`).
            StmtKind::With { items, body } => self.check_with(stmt, items, body, ret, in_loop),

            StmtKind::SetPlace { place, value } => {
                if let Some(root) = place_root_name(place) {
                    self.check_capture_access(root, true)?;
                }
                // Nominal subscript assignment is a method call, not a raw
                // storage projection. Resolve it with the RHS present so
                // overloads, conversions, ownership conventions, origins,
                // aliases, captures, and raising behavior all use the same
                // machinery as explicit `.__setitem__(...)` syntax.
                if self
                    .check_nominal_subscript_assignment(place, value)?
                    .is_some()
                {
                    if let ExprKind::Index { object, .. }
                    | ExprKind::Slice { object, .. }
                    | ExprKind::MultiIndex { object, .. } = &place.kind
                        && let Ok(container) = self.origin_place(object)
                    {
                        self.check_result_carries_container_interior(&container, value)?;
                    }
                    self.record_place_write_invalidation(place.source_span(), place);
                    return Ok(());
                }
                // The place must be a writable location (a field/index chain
                // rooted at a mutable variable or `mut self`); the value must
                // keep the place's type. A width-1 SIMD target (a lane write, or
                // a scalar-alias field) additionally accepts a splatting literal.
                let target = self.check_place(place)?;
                if let ExprKind::Call { name, .. } | ExprKind::MethodCall { method: name, .. } =
                    &place.kind
                {
                    self.check_overwritten_referent(&format!("{name}(...)"), &target)?;
                }
                self.record_place_write_invalidation(place.source_span(), place);
                let storage = self.place_storage_ty(place);
                // The place's type is the assignment's expected context, so a
                // collection display materializes against it (`self.items =
                // [7, 8, 9]` stays `List[Int]` for a List-typed field).
                let found = self.infer_with_expected(value, &target, true)?;
                if let Ok(destination) = self.origin_place(place) {
                    self.check_result_aliases_destination(&destination, value)?;
                }
                // A store into storage that outlives this frame (a parameter
                // or `self` root) must not smuggle a frame-local loan outward
                // — the symmetric twin of the Return escape check. Rebinding a
                // `ref` field counts even when the value type carries no loan:
                // the destination handle itself becomes the loan. Nominal
                // subscript assignment (early-returned above) needs no twin:
                // a `List[ref T]` subscript reaches the referent only
                // (augmented writes), and handle replacement or ref-append is
                // not offered by any overload, so no collection store can
                // install a frame-local handle.
                self.check_outward_store(place, value, &found, &storage)?;
                if let Some(TyKind::Ref(expected_reference)) = storage.as_ref().map(Ty::kind) {
                    let initializes_reference =
                        self.self_initializing && place_root_name(place) == Some("self");
                    if initializes_reference {
                        let actual = self.infer_reference_value(value).ok_or_else(|| {
                            TypeError::TypeMismatch {
                                expected: format!("ref {}", expected_reference.referent),
                                found: found.to_string(),
                                context: "reference field initialization".to_string(),
                            }
                        })?;
                        if !coerces(&actual.referent, &expected_reference.referent)
                            || (expected_reference.mutability
                                == mojito_types::origin::Mutability::Mutable
                                && actual.mutability != mojito_types::origin::Mutability::Mutable)
                        {
                            return Err(TypeError::TypeMismatch {
                                expected: format!("ref {}", expected_reference.referent),
                                found: format!("ref {}", actual.referent),
                                context: "reference field initialization".to_string(),
                            });
                        }
                        self.record_at(value.source_span()).reference_value_use = Some(
                            expected_reference.mutability
                                == mojito_types::origin::Mutability::Mutable,
                        );
                    } else if expected_reference.mutability
                        != mojito_types::origin::Mutability::Mutable
                    {
                        return Err(TypeError::ImmutableBinding(
                            "immutable reference field".to_string(),
                        ));
                    } else {
                        // A write-through replaces the referent's value: run
                        // the referent's consuming/copy analysis, which
                        // out-self initialization deliberately skips — a `^`
                        // transfer stays a move, and a Copyable place read
                        // records its copy so the lowering runs the lifecycle
                        // instead of sharing the source's storage.
                        if !self.storage_conversion_borrows_source(&found, Some(&target)) {
                            self.check_consuming(value, &found, "assignment target")?;
                        }
                    }
                }
                let ok = self.record_implicit_conversion(value, &found, &target)?;
                if !ok {
                    if let Some(mismatch) = self.struct_origin_mismatch(&found, &target) {
                        return Err(mismatch);
                    }
                    return Err(TypeError::TypeMismatch {
                        expected: target.to_string(),
                        found: found.to_string(),
                        context: "assignment target".to_string(),
                    });
                }
                // Existing out-self initialization permits a non-Copyable
                // generic parameter to be installed without spelling a second
                // source-level transfer. Preserve that constructor convention;
                // this marker is needed only for an actual Copyable place read.
                if !matches!(storage.as_ref().map(Ty::kind), Some(TyKind::Ref(_)))
                    && self.is_copyable(&found)
                    && !self.storage_conversion_borrows_source(&found, Some(&target))
                {
                    self.check_consuming(value, &found, "assignment target")?;
                }
                Ok(())
            }

            // `raises` is parsed but its effect is not analyzed (deferred).
            StmtKind::Def { .. } => self.check_def(stmt, false),

            StmtKind::Struct {
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
                enum_cases,
            } => {
                if self.lookup(name).is_some() {
                    return Err(TypeError::Redeclaration(name.clone()));
                }
                let declaration = StructDeclaration {
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
                    enum_cases: enum_cases.as_deref(),
                };
                if self.predeclared_structs.remove(name) {
                    self.check_struct_completion(&declaration)
                } else {
                    self.check_struct(&declaration)
                }
            }

            StmtKind::Trait {
                name,
                refines,
                methods,
                comptime_members,
            } => {
                if self.predeclared_traits.remove(name) {
                    Ok(())
                } else {
                    self.check_trait(name, refines, methods, comptime_members)
                }
            }

            StmtKind::Comptime {
                name,
                type_params,
                ty,
                where_clauses,
                value,
            } => {
                let _position = self.comptime_position();
                if type_params.is_empty()
                    && self.bind_template_comptime(stmt, name, ty.as_ref(), value)?
                {
                    return Ok(());
                }
                // A type name takes the alias route; an alias of a value
                // binder (`comptime lane = dt`), of a local compile-time
                // value (`comptime k = $match<id>`, a `comptime match`
                // binding), of an applied module constant (`comptime E =
                // D`), or of a value alias's application (`comptime n =
                // First[5, 6]`) is the ordinary path below.
                let value_alias = self.names_value_binder(value)
                    || (type_params.is_empty()
                        && (self.names_applied_module_constant(value)
                            || self.applies_value_alias(value)
                            || matches!(&value.kind, ExprKind::Identifier(source)
                                if self.lookup_tparam(source).is_none()
                                    && self.parameter_value_ty(value).is_some())));
                if !value_alias
                    && (!type_params.is_empty()
                        || self.comptime_aliases.contains_key(name)
                        || matches!(
                            value.kind,
                            ExprKind::Identifier(_)
                                | ExprKind::TypeApply { .. }
                                | ExprKind::TypeValue(_)
                        ))
                {
                    // Top-level aliases were registered by `check_program`'s
                    // pre-pass; re-walking the same statement is not a
                    // redeclaration. Any other context still reports its own
                    // rejection (module-scope-only, origin params, ...).
                    if self.function_bases.is_empty() && self.comptime_aliases.contains_key(name) {
                        return Ok(());
                    }
                    return self.check_generic_comptime_alias(
                        name,
                        type_params,
                        ty.as_ref(),
                        where_clauses,
                        value,
                    );
                }
                for condition in where_clauses {
                    let constraint = self.compile_where_clause(condition)?;
                    self.validate_declaration_constraint(name, &constraint)?;
                }
                if ty
                    .as_ref()
                    .is_some_and(super::declarations::is_string_literal_annotation)
                {
                    return Err(TypeError::Unsupported(
                        "cannot construct a value with parametric type: 'StringLiteral[_]'"
                            .to_string(),
                    ));
                }
                // A comptime `Int` is recorded (for value-parameter use) and bound as
                // `Int`. A richer comptime value (tuple/list/string) the `Int` folder
                // can't evaluate is still an ordinary binding — the elaborator has
                // already consumed it for any `comptime for`/`comptime if`.
                // A dtype binding keys later `Scalar[d]`/`SIMD[d, n]`
                // annotations. A symbolic dtype binds as itself, so a
                // `[dt: DType]`-keyed body may name its own lane through one.
                let bound_dtype = match &value.kind {
                    ExprKind::Member { object, field } => {
                        self.dtype_constant(object, field).transpose()?
                    }
                    ExprKind::Identifier(source) => self.dtype_named(source),
                    _ => None,
                };
                if let Some(dtype) = bound_dtype {
                    self.comptime_dtypes
                        .last_mut()
                        .ok_or_else(|| {
                            TypeError::InvariantViolation(
                                "checker scope stack is empty".to_string(),
                            )
                        })?
                        .insert(name.clone(), dtype);
                }
                let annotated = ty
                    .as_ref()
                    .map(|annotation| {
                        self.resolve_storage_annotation(
                            annotation,
                            super::StorageStrictness::AllowBare,
                        )
                    })
                    .transpose()?
                    .filter(|expected| {
                        !matches!(expected.kind(), TyKind::Int | TyKind::IntLiteral)
                    });
                // A module constant was folded before any declaration; only a
                // body's own constant folds here.
                let folded = if self.function_bases.is_empty() && self.scopes.len() == 1 {
                    self.program_constants.own(name).cloned().ok_or(())
                } else {
                    self.eval_ct(value).map_err(drop)
                };
                if let Some(expected) = annotated {
                    // An annotation other than `Int` is the binding's type: the
                    // literal converts to it as a `var` initializer does.
                    if let Ok(v) = folded {
                        self.comptimes.insert(name.clone(), v);
                    }
                    let found = self.infer_with_expected(value, &expected, true)?;
                    if !self.record_implicit_conversion(value, &found, &expected)? {
                        return Err(TypeError::TypeMismatch {
                            expected: expected.to_string(),
                            found: found.to_string(),
                            context: format!("comptime '{name}'"),
                        });
                    }
                    self.declare_immutable(name, expected)?;
                } else if let Ok(v) = folded {
                    self.comptimes.insert(name.clone(), v);
                    self.declare_immutable(name, Ty::int_literal())?;
                } else {
                    let ty = self.infer(value)?;
                    // A compile-time result is a value of its own, copied
                    // out of a reference result as any compile-time
                    // result is (`comptime q = QS[2]`).
                    self.copy_compile_time_result(value, &ty);
                    let declared = self.inferred_binding_ty(&ty, name)?;
                    // The binding is read at run time as its `Float64`
                    // storage type, so the folded literal crosses here.
                    if matches!(ty.kind(), TyKind::FloatLiteral) {
                        self.record_literal_materializations(value, &ty, &declared)?;
                    }
                    self.declare_immutable(name, declared)?;
                }
                // A binding of an application holds it as its attribute,
                // which every read materializes.
                if let Some(application) = self.local_comptime_application(name) {
                    self.record_at(stmt.source_span()).comptime_value = Some(Box::new(application));
                }
                self.record_statement_binding(stmt, name);
                Ok(())
            }

            // A `comptime if` is checked with every arm open and the
            // declaration's parameters symbolic, under source validation and
            // in a template body the executable check keeps: the condition is
            // recorded for the MIR branch, and the elaborator selects. A
            // `comptime for` is checked once with its index symbolic the same
            // way: a kept `range` loop records its bounds for the MIR loop
            // header, and the elaborator unrolls.
            StmtKind::ComptimeIf { branches, orelse } => {
                self.check_conditional(branches, orelse.as_deref(), ret, in_loop, true)
            }
            StmtKind::ComptimeFor { var, iter, body } => {
                self.check_comptime_for(stmt, var, iter, body, ret)
            }
            StmtKind::ComptimeAssert { cond, message } => {
                self.check_comptime_assert(cond, message.as_ref()).map(drop)
            }
            // A kept compile-time block is straight-line code, so what it
            // initializes stays initialized after it.
            StmtKind::Scope(body) => self.check_scoped_block(body, ret, in_loop),

            StmtKind::If { branches, orelse } => {
                self.check_conditional(branches, orelse.as_deref(), ret, in_loop, false)
            }

            StmtKind::Match { subject, cases } => {
                self.check_match(stmt, subject, cases, ret, in_loop)
            }
            StmtKind::ComptimeMatch { subject, cases } => {
                self.check_comptime_match(stmt, subject, cases, ret, in_loop)
            }

            StmtKind::While { cond, body, orelse } => {
                let before = self.uninitialized.borrow().clone();
                if let mojito_ast::ast::Condition::Case { pattern, value } = cond {
                    self.check_case_condition(pattern, value, body, ret, true)?;
                } else {
                    let cond = Self::boolean_condition(cond, "while")?;
                    self.register_named_bindings(cond)?;
                    self.expect_bool(cond, "while condition")?;
                    self.check_scoped_block(body, ret, true)?;
                }
                *self.uninitialized.borrow_mut() = before;
                if let Some(body) = orelse {
                    self.check_scoped_block(body, ret, in_loop)?;
                }
                Ok(())
            }

            // `raise` — its operand must be an `Error` (or a `String`, the
            // shorthand). The raises *effect* (that this must be in a `raises`
            // function or a `try`) is deliberately not analyzed.
            StmtKind::Raise(expr) => {
                self.register_named_bindings(expr)?;
                let ty = self.infer(expr)?;
                let stringish = ty == TyKind::StringLiteral
                    || matches!(ty.kind(), TyKind::Struct(name, args)
                        if args.is_empty() && mojito_symbol::symbol::is_stdlib_string_struct(name));
                let error = if stringish { Ty::error() } else { ty };
                self.require_error("'raise'", error)?;
                Ok(())
            }

            // Imports are parsed but not resolved (no module system yet), so
            // they are a checker no-op — imported names are not made available.
            StmtKind::Import { .. } | StmtKind::FromImport { .. } => Ok(()),

            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                if except.is_some() {
                    self.handled_raise_depth += 1;
                    self.handled_raise_types.borrow_mut().push(Vec::new());
                }
                let body_result = self.check_scoped_block(body, ret, in_loop);
                let handled_types = except.as_ref().map(|_| {
                    self.handled_raise_types
                        .borrow_mut()
                        .pop()
                        .unwrap_or_default()
                });
                if except.is_some() {
                    self.handled_raise_depth -= 1;
                }
                body_result?;
                if let Some((name, ex_body)) = except {
                    self.push_scope();
                    let error = handled_types
                        .as_ref()
                        .and_then(|types| types.first().cloned())
                        .unwrap_or_else(Ty::error);
                    if handled_types
                        .as_ref()
                        .is_some_and(|types| types.iter().any(|candidate| *candidate != error))
                    {
                        self.pop_scope();
                        return Err(TypeError::RaiseTypeMismatch {
                            expected: error.to_string(),
                            found: "multiple error types in one try block".to_string(),
                        });
                    }
                    let result = match name {
                        Some(n) => self.declare(n, error.clone()).and_then(|()| {
                            // The exception target is a real lexical binding. Its
                            // owner is attached to the containing `try` syntax
                            // because the AST stores the target as a name rather
                            // than as an expression node.
                            self.record_statement_binding(stmt, n);
                            self.record_at(stmt.source_span()).binding_ty = Some(error.clone());
                            self.check_block(ex_body, ret, in_loop)
                        }),
                        None => self.check_block(ex_body, ret, in_loop),
                    };
                    self.pop_scope();
                    result?;
                }
                if let Some(body) = orelse {
                    self.check_scoped_block(body, ret, in_loop)?;
                }
                if let Some(body) = finalbody {
                    self.check_scoped_block(body, ret, in_loop)?;
                }
                Ok(())
            }

            StmtKind::For {
                target,
                iter,
                body,
                orelse,
            } => {
                // A destructuring target binds the element to a hidden
                // immutable binder (upstream's `Bind` pattern) and starts the
                // body by destructuring it.
                let hidden = format!("$for{}", target.syntax_id.0);
                let (var, binding, unpack) = match target.loop_binding() {
                    Some((name, mode)) => (name, mode, false),
                    None => (
                        hidden.as_str(),
                        mojito_ast::ast::BindingMode::Immutable,
                        true,
                    ),
                };
                let binding = &binding;
                self.register_named_bindings(iter)?;
                // The loop variable's type comes from the iterable: `Int` for a
                // `range`, the element type for a `List`, or — for a user struct —
                // the element type of its `__iter__()` iterator (`__next__`'s return).
                let iter_ty = self.convert_literal_iterable(iter, self.infer(iter)?);
                let source_mode = Self::iteration_mode(iter);
                let (source, source_mutable) = self.iteration_source(iter);
                let (protocol, raises) = self.loop_site_protocol(
                    &iter_ty,
                    source_mode,
                    *binding,
                    source.as_ref(),
                    source_mutable,
                )?;
                if !raises.is_empty() {
                    self.raising_iterations
                        .borrow_mut()
                        .insert(iter.source_span());
                }
                for (operation, error) in raises {
                    self.require_error(operation, error)?;
                }
                let binding_plan = protocol.binding.as_deref().cloned().ok_or_else(|| {
                    TypeError::InvariantViolation("a loop protocol lost its binding plan".into())
                })?;
                if source_mode == mojito_checked::checked::IterationMode::Owned
                    && binding_plan.action
                        == mojito_checked::checked::IterationBindingAction::MoveValue
                    && !self.is_deinitable(&binding_plan.binding_ty)
                    && block_can_escape_owned_iteration(body, 0)
                {
                    // Name the element's declared obligation so the rejection
                    // says what each residual element still requires.
                    let obligation = self.residual_obligation_suffix(&binding_plan.binding_ty);
                    return Err(TypeError::Unsupported(format!(
                        "owned iteration over non-Deinitable '{}' cannot exit early; its residual elements would require explicit destruction{obligation}",
                        binding_plan.binding_ty
                    )));
                }
                // A linear-element body must also be free of unhandled raising
                // calls: the syntactic walk above sees only `raise` statements,
                // while a propagating call error abandons the residuals just
                // the same. Observed while the body checks (a `try`-handled
                // call is contained and does not mark).
                let linear_element = (source_mode == mojito_checked::checked::IterationMode::Owned
                    && binding_plan.action
                        == mojito_checked::checked::IterationBindingAction::MoveValue
                    && !self.is_deinitable(&binding_plan.binding_ty))
                .then(|| binding_plan.binding_ty.clone());
                let binding_ty = binding_plan.binding_ty.clone();
                self.record_at(iter.source_span()).iteration_protocol = Some(Box::new(protocol));
                self.push_scope();
                self.record_at(stmt.source_span()).binding_ty = Some(binding_ty.clone());
                if self.is_deinitable(&binding_ty) {
                    self.explicit_destroy_deletability
                        .borrow_mut()
                        .bindings
                        .insert(stmt.source_span());
                } else if matches!(binding_ty.kind(), TyKind::Param { .. }) {
                    self.explicit_destroy_deletability
                        .borrow_mut()
                        .linear_bindings
                        .insert(stmt.source_span());
                }
                if linear_element.is_some() {
                    let baseline = self.handled_raise_depth;
                    self.raise_observation_frames
                        .borrow_mut()
                        .push((baseline, false));
                }
                let result = (|| {
                    self.declare_with_mutability(var, binding_ty, binding_plan.mutable)?;
                    self.record_statement_binding(stmt, var);
                    if unpack {
                        self.check_loop_target(target, var, ret, false)?;
                    }
                    self.check_block(body, ret, true)
                })();
                self.pop_scope();
                let raised = linear_element.is_some()
                    && self
                        .raise_observation_frames
                        .borrow_mut()
                        .pop()
                        .is_some_and(|(_, flag)| flag);
                result?;
                if raised && let Some(element) = linear_element {
                    let obligation = self.residual_obligation_suffix(&element);
                    return Err(TypeError::Unsupported(format!(
                        "owned iteration over non-Deinitable '{element}' cannot contain an unhandled raising call; a propagating error would abandon its residual elements{obligation}",
                    )));
                }
                if let Some(body) = orelse {
                    self.check_scoped_block(body, ret, in_loop)?;
                }
                Ok(())
            }

            StmtKind::Break => {
                if in_loop {
                    Ok(())
                } else {
                    Err(TypeError::BreakOutsideLoop)
                }
            }

            StmtKind::Continue => {
                if in_loop {
                    Ok(())
                } else {
                    Err(TypeError::ContinueOutsideLoop)
                }
            }

            StmtKind::Return(expr) => {
                if let Some(expression) = expr {
                    self.register_named_bindings(expression)?;
                }
                let Some(expected) = ret else {
                    return Err(TypeError::ReturnOutsideFunction);
                };
                let found = match expr {
                    // A struct origin tail is judged against the return
                    // annotation resolved over the body's own places
                    // (`-> View[origin_of(self.items)]` takes a view over
                    // `self.items`, not one over `self`) and then carries
                    // the signature's spelling of that origin.
                    Some(e) => self.reconcile_return_origin_tails(
                        &self.infer_with_expected(e, expected, true)?,
                        expected,
                    )?,
                    None if self.named_result_context.last() == Some(&true) => expected.clone(),
                    None => Ty::none(),
                };
                // Record the return-position conversion before the escape
                // check: a view-constructor implicit conversion installs the
                // source-borrow fact that makes `aggregate_origins` see the
                // frame-local loan behind the converted spelling.
                let compatible = match expr {
                    Some(expression) => {
                        self.record_implicit_conversion(expression, &found, expected)?
                    }
                    None => self.value_coerces(&found, expected),
                };
                if let Some(expression) = expr
                    && !matches!(expected.kind(), TyKind::Ref(_))
                    && (self.type_may_carry_loans(expected) || self.type_may_carry_loans(&found))
                    && self
                        .aggregate_origins(expression)
                        .iter()
                        .any(|origin| self.aggregate_origin_escapes(origin))
                {
                    // A bare pointer return gets the pointer-specific
                    // diagnostic; an aggregate may mix reference and pointer
                    // loans and keeps the reference wording.
                    if matches!(found.kind(), TyKind::Pointer { .. }) {
                        return Err(TypeError::PointerEscapesOrigin);
                    }
                    return Err(TypeError::ReturnsReferenceToLocal);
                }
                if let (Some(e), Some(Some((signature, parameter_owners, self_owner)))) =
                    (expr, self.return_ref_contracts.last())
                {
                    let actual = if let Some(origin) = self.returned_reference_parameter_origin(e) {
                        origin
                    } else {
                        let supplied = self.reference_actual(e)?.origin;
                        self.returned_pointer_dereference_origin(e, self_owner.as_ref())
                            .unwrap_or(supplied)
                    };
                    let parameter_origins: Vec<_> = parameter_owners
                        .iter()
                        .map(|owner| {
                            Some(mojito_types::origin::Origin::Place(
                                mojito_types::origin::OriginPlace {
                                    root: *owner,
                                    path: Vec::new(),
                                },
                            ))
                        })
                        .collect();
                    let allowed = substitute_sig_origin_with_self(
                        &signature.origin,
                        &parameter_origins,
                        self_owner.clone().map(mojito_types::origin::Origin::Place),
                    );
                    // Bundled collections intentionally abstract their private
                    // backing storage behind a public owned-interior region:
                    // List bridges its raw pointer slot to `element`, while Dict
                    // bridges its entries List to the replace-on-lookup `value`
                    // generation. Callers inherit the public region, never the
                    // implementation storage origin. `Tuple`, `MaybeUninit` and
                    // `Variant` read a compiler-private storage field where
                    // upstream casts a pointer carrying `self`'s origin. Keep
                    // this privilege at the return boundary, restricted to
                    // compiler-shipped source paths.
                    let bundled_private_storage_bridge =
                        (self.self_ty.as_ref().is_some_and(|ty| {
                            list_element(ty).is_some()
                                || dict_elements(ty).is_some()
                                || array_element(ty).is_some()
                                || mojito_types::types::optional_element(ty).is_some()
                                || mojito_types::types::owned_pointer_element(ty).is_some()
                        }) && is_bundled_collection_source(e.source.as_deref()))
                            || is_bundled_private_storage_source(e.source.as_deref());
                    if !bundled_private_storage_bridge {
                        // The returned place must be the declared origin
                        // itself: a field of a declared owner, or an element of
                        // a declared container, is a different origin.
                        if !origin_is_within(&actual, &allowed) {
                            return Err(TypeError::ReturnsReferenceToLocal);
                        }
                        if !origin_is_declared(&actual, &allowed) {
                            return Err(TypeError::ReturnOriginIncompatible);
                        }
                    }
                }
                // `expected` is the referent type for a reference-returning
                // declaration; the reference contract is retained separately.
                // Returning a place through that checked contract borrows it —
                // it does not copy/consume the referent.
                let returning_reference = self
                    .return_ref_contracts
                    .last()
                    .is_some_and(Option::is_some);
                if returning_reference && let Some(expression) = expr {
                    self.record_at(expression.source_span()).reference_value_use = Some(false);
                }
                if let Some(e) = expr
                    && !returning_reference
                    && !self.storage_conversion_borrows_source(&found, Some(expected))
                {
                    self.check_consuming(e, &found, "return value")?;
                }
                // Returning a callable *value*, or an aggregate storing a
                // closure, is an escape; returning a checked reference to
                // callable storage is not. The latter is how a nominal Tuple's
                // indexed accessor exposes a function element while the Tuple
                // owner remains live.
                if !returning_reference
                    && (matches!(
                        found.kind(),
                        TyKind::Func(_) | TyKind::GenericFunc(_) | TyKind::Overload(_)
                    ) || holds_closure(&found))
                {
                    return Err(TypeError::ClosureEscape);
                }
                if !compatible {
                    return Err(self
                        .struct_origin_mismatch(&found, expected)
                        .unwrap_or_else(|| TypeError::TypeMismatch {
                            expected: expected.to_string(),
                            found: found.to_string(),
                            context: "return".to_string(),
                        }));
                }
                Ok(())
            }

            StmtKind::Pass => Ok(()),

            StmtKind::Expr(expr) => {
                self.register_named_bindings(expr)?;
                self.infer(expr)?;
                // An expression statement discards its value: a reference
                // result is not read, so it requires no copy.
                self.discarded_reference_results
                    .borrow_mut()
                    .insert(expr.source_span());
                self.record_unconsumed_temporary(expr);
                self.record_discarded_transfer(expr);
                Ok(())
            }
        }
    }

    /// Check one function declaration: an ordinary (possibly nested) `def`
    /// statement, or (`lambda = true`) the hidden definition synthesized for a
    /// lambda expression. Lambda mode differs only where current Mojo differs:
    /// an omitted capture list imm-captures free variables by default, an
    /// explicit capture-all convention or a lambda-owned parameter list forces
    /// a capturing environment, and the fixed-`None` omitted return type is
    /// named in the body mismatch diagnostic.
    ///
    /// A function's own `Origin` binders resolve in its signature and body
    /// exactly as a method's do (`enclosing_origin_param`), indexed after any
    /// enclosing struct and method binders.
    pub(super) fn check_def(&mut self, stmt: &Stmt, lambda: bool) -> Result<(), TypeError> {
        let StmtKind::Def {
            type_params,
            params,
            ..
        } = &stmt.kind
        else {
            return self.check_def_inner(stmt, lambda);
        };
        let enclosing = self.enclosing_type_params.len();
        self.enclosing_type_params
            .extend(type_params.iter().cloned());
        // A parameter named `self` is an ordinary binding, whose own
        // convention decides whether it is writable.
        let saved_self_mutable = self.self_mutable;
        if params.iter().any(|param| param.name == "self") {
            self.self_mutable = true;
        }
        let result = self.check_def_inner(stmt, lambda);
        self.self_mutable = saved_self_mutable;
        self.enclosing_type_params.truncate(enclosing);
        result
    }

    #[allow(
        clippy::cognitive_complexity,
        clippy::too_many_lines,
        reason = "TODO: split this pass"
    )]
    fn check_def_inner(&mut self, stmt: &Stmt, lambda: bool) -> Result<(), TypeError> {
        let StmtKind::Def {
            name,
            type_params,
            params,
            positional_only,
            keyword_only,
            captures,
            ret: ret_anno,
            body,
            raises,
            raises_type,
            decorators,
            where_clauses,
        } = &stmt.kind
        else {
            return Err(TypeError::InvariantViolation(
                "check_def requires a function declaration".to_string(),
            ));
        };
        if lambda && self.function_bases.is_empty() {
            return Err(TypeError::Unsupported(
                "lambda expressions outside a function body".to_string(),
            ));
        }
        reject_default_lambdas(params)?;
        if RESERVED_FUNCTION_NAMES.contains(&name.as_str()) {
            return Err(TypeError::ReservedName(name.clone()));
        }
        if self.structs.contains_key(name) {
            return Err(TypeError::Redeclaration(name.clone()));
        }
        // Current Mojo restricts the `deinit` convention to struct method
        // arguments; free functions, nested functions, and lambdas take
        // ownership with `var` instead. A trait member is a method of its
        // trait, whose receiver may be `deinit`.
        let trait_member = std::mem::take(&mut self.checking_trait_member);
        let unavailable = if trait_member {
            None
        } else {
            self.check_decorators(DecoratorSite::Function, decorators)?
                .unavailable
                .map(|unavailable| unavailable.message(name))
        };
        // An unavailable function declares a signature only: its body is
        // the one `...` upstream requires.
        if unavailable.is_some()
            && body
                .iter()
                .any(|statement| !matches!(statement.kind, StmtKind::Pass))
        {
            return Err(TypeError::InvalidDecorator(
                "unexpected function body in @unavailable function declaration, use '...'"
                    .to_string(),
            ));
        }
        if let Some(param) = params
            .iter()
            .skip(usize::from(trait_member))
            .find(|p| p.convention == Some(mojito_ast::ast::ArgConvention::Deinit))
        {
            return Err(TypeError::Unsupported(format!(
                "the 'deinit' convention on parameter '{}' is only valid on struct \
                 method arguments; take ownership with 'var' instead",
                param.name
            )));
        }
        // Free functions, including generic functions, share one binder
        // for regular, `*args`, and homogeneous `**kwargs` parameters.
        // A `*args` variadic is supported on non-generic functions; any
        // regular parameters after it are keyword-only.
        let variadic_idx = params
            .iter()
            .position(|p| p.kind == mojito_ast::ast::ParamKind::Variadic);
        let kw_variadic_idx = params
            .iter()
            .position(|p| p.kind == mojito_ast::ast::ParamKind::KwVariadic);
        // Regular (non-variadic) parameters, over which arity is computed.
        let regular: Vec<&mojito_ast::ast::FnParam> = params
            .iter()
            .filter(|p| p.kind == mojito_ast::ast::ParamKind::Regular)
            .collect();
        let out_params: Vec<_> = regular
            .iter()
            .copied()
            .filter(|p| matches!(p.convention, Some(mojito_ast::ast::ArgConvention::Out)))
            .collect();
        if out_params.len() > 1 {
            return Err(TypeError::Unsupported(
                "multiple named 'out' results".to_string(),
            ));
        }
        let named_result = out_params.first().copied();
        if named_result.is_some() && ret_anno.is_some() {
            return Err(TypeError::Unsupported(
                "a function cannot declare both a named result and '->' return type".to_string(),
            ));
        }
        let caller_regular: Vec<_> = regular
            .iter()
            .copied()
            .filter(|p| !matches!(p.convention, Some(mojito_ast::ast::ArgConvention::Out)))
            .collect();
        let pos_only = regular_marker_index(params, *positional_only);
        let kw_only = effective_keyword_only_index(params, *keyword_only, variadic_idx);
        let required = required_mask(&caller_regular, kw_only)?;
        self.validate_origin_signature(type_params, params, None)?;
        // Two overloads of one name are two declarations, so each owns its
        // binders under the symbol it lowers to.
        let owner = if self.function_bases.is_empty() {
            mojito_symbol::symbol::lowered_def_name(
                name,
                type_params,
                params,
                &*self.overload_sets,
                &|name| self.program_constants.get(name),
            )
        } else {
            name.clone()
        };
        let mut decls = self.classify_params(&owner, type_params)?;
        let mut function_assumptions = HashSet::new();
        let mut erased_origin_constraints = Vec::new();
        // An arithmetic `where` operand names the declaration's own value
        // parameters, so their scope is open while the clauses compile.
        self.push_param_scope(&decls);
        let compiled = where_clauses
            .iter()
            .map(|condition| {
                self.compile_where_clause(condition)
                    .map(|constraint| self.bind_constraint(&constraint, &decls))
            })
            .collect::<Result<Vec<_>, _>>();
        self.tparams.pop();
        for constraint in compiled? {
            guaranteed_conformance_atoms(&constraint, &mut function_assumptions);
            if let Some(last) = decls.last_mut() {
                match last {
                    ParamDecl::Type { constraints, .. } | ParamDecl::Value { constraints, .. } => {
                        constraints.push(constraint);
                    }
                }
            } else if type_params.is_empty() {
                self.validate_declaration_constraint(name, &constraint)?;
            } else {
                erased_origin_constraints.push(constraint);
            }
        }
        self.generic_parameters.borrow_mut().insert(
            mojito_checked::checked::GenericSite::Function {
                module: stmt.module.clone(),
                declaration: stmt.span,
                syntax: stmt.syntax_id,
            },
            decls.clone(),
        );
        // Type parameters are in scope while resolving the signature and
        // checking the body (as bare `T`), and value parameters as the typed
        // references a dependent parameter expression names (`Buf[n + 1]`).
        self.push_param_scope(&decls);

        let signature = (|| {
            let generated = self.generated_declaration.replace(name.contains('$'));
            let param_tys = self.param_tys(params);
            self.generated_declaration.set(generated);
            let param_tys = param_tys?;
            let ret_ty = match (ret_anno, named_result) {
                (Some(SourceType::Ref { referent, .. }), _) => self.ty_from_anno(referent)?,
                // A compiler-generated specialization (`$`-mangled) rebinds
                // already-checked annotations whose origin identity is
                // legitimately erased; signature concreteness applies to
                // user-spelled declarations only.
                (Some(t), _) if name.contains('$') => {
                    self.resolve_generated_return_annotation(t)?
                }
                (Some(t), _) => self.resolve_return_annotation(t)?,
                (None, Some(result)) => self.ty_from_anno(&result.ty)?,
                (None, None) => Ty::none(),
            };
            if !name.contains('$') {
                self.check_result_origins(ret_anno.as_ref(), params, &param_tys, None)?;
            }
            Ok::<_, TypeError>((param_tys, ret_ty))
        })();
        let (param_tys, ret_ty) = match signature {
            Ok(sig) => sig,
            Err(e) => {
                self.tparams.pop();
                return Err(e);
            }
        };
        let ref_params = lower_ref_param_sigs(type_params, &caller_regular, 0)?;
        let ref_return = match ret_anno {
            Some(SourceType::Ref { origin, .. }) => Some(self.lower_ref_sig_resolved(
                origin.as_ref().ok_or_else(|| {
                    TypeError::Unsupported("reference return requires an origin".to_string())
                })?,
                type_params,
                &regular,
                0,
            )?),
            _ => None,
        };
        for (param, ty) in param_tys.iter().enumerate() {
            self.declaration_types.borrow_mut().insert(
                mojito_checked::checked::AnnotationSite::FunctionParam {
                    module: stmt.module.clone(),
                    declaration: stmt.span,
                    syntax: stmt.syntax_id,
                    param,
                },
                ty.clone(),
            );
        }
        self.declaration_types.borrow_mut().insert(
            mojito_checked::checked::AnnotationSite::FunctionReturn {
                module: stmt.module.clone(),
                declaration: stmt.span,
                syntax: stmt.syntax_id,
            },
            ret_ty.clone(),
        );
        if let Err(error) = self.check_parameter_defaults(params, &param_tys, &decls) {
            self.tparams.pop();
            return Err(error);
        }

        // Bind the function in the enclosing scope before checking its
        // body, so it can call itself (recursion). A generic `def`
        // becomes a `GenericFunc` (its call sites infer/supply parameters).
        let declared_error = self.declared_error(*raises, raises_type.as_ref())?;
        let effect_raises = declared_error
            .as_ref()
            .is_some_and(|ty| *ty != TyKind::Never);
        let parameter_closure = mojito_ast::ast::is_parameter_closure(decorators);
        let initial_environment = if parameter_closure {
            mojito_types::origin::CallableEnvironment::Capturing(
                mojito_types::origin::CaptureOriginSet::Infer,
            )
        } else if self.function_bases.is_empty() {
            mojito_types::origin::CallableEnvironment::Thin
        } else {
            mojito_types::origin::CallableEnvironment::Default
        };
        self.declaration_effects.borrow_mut().insert(
            mojito_checked::checked::AnnotationSite::FunctionReturn {
                module: stmt.module.clone(),
                declaration: stmt.span,
                syntax: stmt.syntax_id,
            },
            mojito_checked::checked::DeclarationEffect {
                raises: effect_raises,
                error: effect_raises.then(|| declared_error.clone()).flatten(),
                returns_reference: ref_return.is_some(),
                param_writes: mojito_checked::checked::DeclarationEffect::param_writes(
                    &caller_regular
                        .iter()
                        .map(|parameter| parameter.convention)
                        .collect::<Vec<_>>(),
                    &ref_params,
                ),
                availability: Vec::new(),
            },
        );
        let fn_ty = if decls.is_empty() {
            let regular_tys: Vec<Ty> = params
                .iter()
                .zip(&param_tys)
                .filter(|(p, _)| {
                    p.kind == mojito_ast::ast::ParamKind::Regular
                        && !matches!(p.convention, Some(mojito_ast::ast::ArgConvention::Out))
                })
                .map(|(_, ty)| ty.clone())
                .collect();
            TyKind::Func(Box::new(FuncTy {
                environment: initial_environment,
                params: regular_tys,
                names: caller_regular.iter().map(|p| p.name.clone()).collect(),
                ret: ret_ty.clone(),
                required,
                variadic: variadic_idx.map(|vi| param_tys[vi].clone()),
                kw_variadic: kw_variadic_idx.map(|index| param_tys[index].clone()),
                positional_only: pos_only,
                keyword_only: kw_only,
                raises: effect_raises,
                error: declared_error.clone(),
                conventions: caller_regular.iter().map(|p| p.convention).collect(),
                ref_params,
                ref_return: ref_return.clone().map(Box::new),
                transfers: TransferSet::default(),
            }))
        } else {
            let regular_tys: Vec<Ty> = params
                .iter()
                .zip(&param_tys)
                .filter(|(p, _)| {
                    p.kind == mojito_ast::ast::ParamKind::Regular
                        && !matches!(p.convention, Some(mojito_ast::ast::ArgConvention::Out))
                })
                .map(|(_, ty)| ty.clone())
                .collect();
            TyKind::GenericFunc(Box::new(GenericFuncTy {
                decls: decls.clone(),
                func: FuncTy {
                    environment: initial_environment,
                    params: regular_tys,
                    names: caller_regular.iter().map(|p| p.name.clone()).collect(),
                    ret: ret_ty.clone(),
                    required,
                    variadic: variadic_idx.map(|vi| param_tys[vi].clone()),
                    kw_variadic: kw_variadic_idx.map(|index| param_tys[index].clone()),
                    positional_only: pos_only,
                    keyword_only: kw_only,
                    raises: effect_raises,
                    error: declared_error.clone(),
                    conventions: caller_regular.iter().map(|p| p.convention).collect(),
                    ref_params,
                    ref_return: ref_return.clone().map(Box::new),
                    transfers: TransferSet::default(),
                },
            }))
        };
        let fn_ty = Ty::new(fn_ty);
        self.declaration_types.borrow_mut().insert(
            mojito_checked::checked::AnnotationSite::FunctionType {
                module: stmt.module.clone(),
                declaration: stmt.span,
                syntax: stmt.syntax_id,
            },
            fn_ty.clone(),
        );
        // A function's name is not a variable: assigning it is rejected.
        if let Err(e) = self.declare_immutable(name, fn_ty.clone()) {
            self.tparams.pop();
            return Err(e);
        }
        self.record_positional_collector(
            name,
            variadic_idx.map(|index| {
                (
                    params[index].name.as_str(),
                    matches!(
                        params[index].convention,
                        Some(mojito_ast::ast::ArgConvention::Var)
                    ),
                )
            }),
        );
        self.record_statement_binding(stmt, name);
        let mut origin_signature =
            callable_origin_signature(type_params, &caller_regular, erased_origin_constraints);
        origin_signature.view_return =
            self.view_return_origins(ret_anno.as_ref(), type_params, &regular, 0, None);
        origin_signature.unavailable.clone_from(&unavailable);
        self.register_callable_origins(name, origin_signature);
        let capture_policy = if self.function_bases.is_empty() {
            if captures.is_some() {
                self.tparams.pop();
                return Err(TypeError::Unsupported(
                    "unified capture lists are valid only on nested functions".to_string(),
                ));
            }
            None
        } else {
            let mut entries = HashMap::new();
            let mut checked_captures = Vec::new();
            if let Some(captures) = captures {
                for capture in &captures.entries {
                    if let Err(error) = self.check_capture_access(&capture.name, false) {
                        self.tparams.pop();
                        return Err(error);
                    }
                    let Some(scope) = self.binding_scope(&capture.name) else {
                        self.tparams.pop();
                        return Err(TypeError::UndefinedVariable(capture.name.clone()));
                    };
                    if scope == 0 {
                        self.tparams.pop();
                        return Err(TypeError::Unsupported(format!(
                            "module binding '{}' is not a closure capture",
                            capture.name
                        )));
                    }
                    if capture.kind == mojito_ast::ast::CaptureKind::Mut
                        && !self.is_binding_mutable(&capture.name)
                    {
                        self.tparams.pop();
                        return Err(TypeError::ImmutableBinding(capture.name.clone()));
                    }
                    if let Err(error) = self.check_capture_capability(&capture.name, capture.kind) {
                        self.tparams.pop();
                        return Err(error);
                    }
                    let binding = self.lookup_owner(&capture.name).ok_or_else(|| {
                        TypeError::InvariantViolation(format!(
                            "capture '{}' lost its checked binding",
                            capture.name
                        ))
                    })?;
                    let ty = self.lookup(&capture.name).cloned().ok_or_else(|| {
                        TypeError::InvariantViolation(format!(
                            "capture '{}' lost its checked storage type",
                            capture.name
                        ))
                    })?;
                    checked_captures.push(self.checked_capture(
                        &capture.name,
                        binding,
                        ty,
                        capture.kind,
                    ));
                    entries.insert(capture.name.clone(), capture.kind);
                }
            }
            self.record_at(stmt.source_span()).captures = Some(checked_captures);
            Some(CapturePolicy {
                base: self.scopes.len(),
                function_name: name.clone(),
                declaration: stmt.source_span(),
                entries,
                // A lambda with the capture list omitted entirely imm-captures
                // its free variables; an explicit `{}` keeps the no-default
                // policy and rejects them.
                default: captures.as_ref().and_then(|list| list.default).or_else(|| {
                    (lambda && captures.is_none()).then_some(mojito_ast::ast::CaptureKind::Imm)
                }),
                // A `@__parameter` closure (which takes no capture list)
                // captures each free variable by its binding's mutability, as
                // upstream: `mut` for a mutable local, `imm` otherwise.
                implicit_by_binding: parameter_closure && captures.is_none(),
                lambda,
            })
        };
        self.assume_declared_propositions(&decls);
        self.assumed_conformances
            .borrow_mut()
            .push(function_assumptions);
        // A parameter typed by a dependent pack projection (`values.Ts[index]`
        // in a `Tuple.consume_elements` handler) is opaque inside the user's
        // handler body, as upstream: the pack's `Movable` bound is all the
        // body may use of it. Generated (`$`) clones bind the concrete
        // element instead. An element of a pack in scope (`x: Ts[0]`), or of
        // a type's raw-list member (`x: TypeList[Trait=AnyType,
        // T._types]()[0]`), keeps its dependent type, whose capabilities are
        // the list's element trait.
        let opaque_params: Vec<Option<Ty>> = params
            .iter()
            .zip(&param_tys)
            .map(|(param, ty)| {
                (!name.contains('$')
                    && param.kind == mojito_ast::ast::ParamKind::Regular
                    && matches!(ty.kind(), TyKind::Dependent(dependent)
                    if !dependent.pack_element().is_some_and(|(list, _)| {
                        self.pack_in_scope(list)
                            || matches!(
                                list.kind(),
                                mojito_types::param_expr::ParamKind::TypeMember { .. }
                            )
                    })))
                .then(|| {
                    TyKind::Param {
                        binder: synthetic_binder(&dependent_projection_spelling(&param.ty)),
                        bounds: vec!["Movable".to_string()],
                        callable_bound: None,
                    }
                    .into()
                })
            })
            .collect();
        for (param, (ty, opaque)) in param_tys.iter().zip(&opaque_params).enumerate() {
            let site = mojito_checked::checked::AnnotationSite::FunctionParam {
                module: stmt.module.clone(),
                declaration: stmt.span,
                syntax: stmt.syntax_id,
                param,
            };
            let bound_ty = opaque.as_ref().unwrap_or(ty);
            let mut deletability = self.explicit_destroy_deletability.borrow_mut();
            // A type parameter whose bounds do not prove `Deinitable` makes an
            // owned parameter linear, as upstream — except a positional
            // variadic pack (`var *values: T`), whose elements upstream
            // destroys implicitly (a `**kwargs` pack stays linear).
            let positional_pack = params[param].kind == mojito_ast::ast::ParamKind::Variadic;
            if matches!(bound_ty.kind(), TyKind::Param { .. })
                && !self.is_deinitable(bound_ty)
                && !positional_pack
            {
                deletability.linear_declarations.insert(site);
            } else if self.is_deinitable(bound_ty) {
                deletability.declarations.insert(site);
            }
        }
        let module_level = self.function_bases.is_empty() && !lambda;
        self.push_scope();
        self.function_bases.push(self.scopes.len() - 1);
        if let Some(policy) = capture_policy {
            self.capture_contexts.borrow_mut().push(policy);
        }
        self.raising_context.push(declared_error);
        let mut result = Ok(());
        // A module-level body is a carry site: its parameter bindings, its
        // frames, and its inference are what the previous pass recorded for
        // it, or what this pass records.
        let site = (module_level && self.carries_bodies()).then(|| stmt.source_span());
        let carried = site.as_ref().is_some_and(|key| self.carry_body(key, name));
        let site_start = if carried {
            None
        } else {
            site.as_ref().map(|key| self.enter_body_site(key))
        };
        if carried {
            // Nothing to bind or infer: the pops below balance the pushes above.
        } else {
            // Value parameters are ordinary `Int` locals in the body.
            for d in &decls {
                if let ParamDecl::Value { name, ty, .. } = d {
                    result = self.declare_value_parameter(
                        name.trim_start_matches('*'),
                        if matches!(d, ParamDecl::Value { variadic: true, .. }) {
                            TyKind::VariadicPack(ty.clone()).into()
                        } else {
                            (*ty).clone()
                        },
                    );
                    if result.is_err() {
                        break;
                    }
                }
            }
            if result.is_ok() {
                for ((param, ty), opaque) in params.iter().zip(&param_tys).zip(&opaque_params) {
                    let bind_ty = match param.kind {
                        mojito_ast::ast::ParamKind::Variadic => {
                            mojito_types::types::variadic_collector_type(
                                ty.clone(),
                                param.convention,
                            )
                        }
                        mojito_ast::ast::ParamKind::KwVariadic => self.kwargs_collector_ty(
                            ty.clone(),
                            &format!("keyword collector '{}'", param.name),
                        )?,
                        mojito_ast::ast::ParamKind::Regular => {
                            opaque.clone().unwrap_or_else(|| ty.clone())
                        }
                    };
                    // Duplicate parameter names are a redeclaration.
                    result = self.declare_with_mutability(
                        &param.name,
                        bind_ty.clone(),
                        param.kind == mojito_ast::ast::ParamKind::KwVariadic
                            || matches!(
                                param.convention,
                                Some(mojito_ast::ast::ArgConvention::Out)
                            )
                            || ref_parameter_is_writable(param, type_params),
                    );
                    if result.is_ok() {
                        self.record_owned_pack(&param.name, param.kind, param.convention);
                    }
                    if result.is_ok()
                        && matches!(param.convention, Some(mojito_ast::ast::ArgConvention::Ref))
                    {
                        let binder =
                            self.reference_parameter_struct_binder(param.origin.as_deref());
                        self.register_reference_parameter(
                            &param.name,
                            bind_ty.clone(),
                            ref_parameter_is_writable(param, type_params),
                            binder,
                        );
                    }
                    if result.is_ok()
                        && !matches!(bind_ty.kind(), TyKind::Ref(_))
                        && self.type_carries_loans(&bind_ty)
                        && let Some(owner) = self.lookup_owner(&param.name)
                    {
                        self.set_aggregate_origins(
                            &param.name,
                            vec![mojito_types::origin::Origin::Place(
                                mojito_types::origin::OriginPlace {
                                    root: owner,
                                    path: Vec::new(),
                                },
                            )],
                        );
                    }
                    if result.is_err() {
                        break;
                    }
                }
            }
            // A function body is a fresh loop context: `break`/`continue`
            // do not cross into a nested `def`.
            if result.is_ok() {
                let owners: Vec<_> = caller_regular
                    .iter()
                    .map(|param| {
                        self.lookup_owner(&param.name)
                            .expect("bound function parameter")
                    })
                    .collect();
                let base = *self
                    .function_bases
                    .last()
                    .expect("function scope is active");
                let mut allowed: std::collections::HashSet<_> = owners.iter().copied().collect();
                // Variadic collectors are parameters too (see the method
                // twin in declarations.rs).
                allowed.extend(
                    params
                        .iter()
                        .filter(|param| param.kind != mojito_ast::ast::ParamKind::Regular)
                        .filter_map(|param| self.lookup_owner(&param.name)),
                );
                // A nested def can reach the enclosing callable's outliving
                // storage (`self`, parameters) through captures; stores
                // into those owners face the same store-outward rule, with
                // this def's own frame deciding source locality.
                if let Some((_, enclosing)) = self.aggregate_escape_contexts.last() {
                    allowed.extend(enclosing.iter().copied());
                }
                self.aggregate_escape_contexts.push((base, allowed));
                // A nested `def`'s bindings, its `out` result among them, in
                // declaration order.
                if !module_level && !lambda {
                    let declared = regular
                        .iter()
                        .map(|param| {
                            self.lookup_owner(&param.name)
                                .expect("bound function parameter")
                        })
                        .collect();
                    self.nested_def_params
                        .borrow_mut()
                        .insert(stmt.source_span(), declared);
                }
                self.transfer_frames.borrow_mut().push(TransferFrame {
                    callable: name.clone(),
                    param_owners: owners.clone(),
                    param_borrowed: caller_regular
                        .iter()
                        .map(|param| {
                            matches!(
                                param.convention,
                                Some(
                                    mojito_ast::ast::ArgConvention::Mut
                                        | mojito_ast::ast::ArgConvention::Ref
                                )
                            )
                        })
                        .collect(),
                    self_owner: None,
                    value_callables: decls
                        .iter()
                        .filter_map(|decl| match decl {
                            ParamDecl::Value { name, ty, .. }
                                if matches!(
                                    *ty.kind(),
                                    TyKind::Func(_) | TyKind::GenericFunc(_)
                                ) =>
                            {
                                Some(name.trim_start_matches('*').to_string())
                            }
                            _ => None,
                        })
                        .collect(),
                    effects: Vec::new(),
                    recorded: Vec::new(),
                    latent_escapes: false,
                    call_throughs: Vec::new(),
                });
                self.raise_observation_frames
                    .borrow_mut()
                    .push((self.handled_raise_depth, false));
                self.return_ref_contracts
                    .push(ref_return.map(|signature| (signature, owners, None)));
                self.return_annotations
                    .push(Self::body_return_annotation(ret_anno.as_ref(), name));
                self.named_result_context.push(named_result.is_some());
                if module_level {
                    self.pack_element_views.borrow_mut().clear();
                }
                count_body_inference(!decls.is_empty(), || name.clone());
                // A nested `def` or a lambda lifts under its enclosing
                // declaration.
                let enclosing = if module_level {
                    self.lifting_owner.replace(owner.clone())
                } else {
                    self.lifting_owner.clone()
                };
                result = self.check_block(body, Some(&ret_ty), false);
                self.lifting_owner = enclosing;
                self.named_result_context.pop();
                self.return_annotations.pop();
                self.return_ref_contracts.pop();
                self.raise_observation_frames.borrow_mut().pop();
                if let Some(frame) = self.transfer_frames.borrow_mut().pop() {
                    if module_level {
                        self.top_level_callables
                            .borrow_mut()
                            .insert(frame.callable.clone());
                    }
                    if !frame.effects.is_empty() {
                        self.transfer_effects
                            .borrow_mut()
                            .insert(frame.callable.clone(), frame.effects);
                    }
                    if !frame.call_throughs.is_empty() {
                        self.call_through_effects
                            .borrow_mut()
                            .insert(frame.callable, frame.call_throughs);
                    }
                }
                self.aggregate_escape_contexts.pop();
            }
        }
        if let (Some(key), Some(start)) = (site, site_start) {
            self.leave_body_site(key, name, &start);
        }
        // A function with a non-`None` return type must return on every
        // path (falling off the end would yield `None`).
        let reach = |stmt: &Stmt| self.match_reach(stmt);
        if result.is_ok()
            && unavailable.is_none()
            && named_result.is_none()
            && ret_ty != TyKind::None
            && !definitely_returns(body, &reach)
        {
            result = Err(TypeError::MissingReturn(name.clone()));
        }
        if result.is_ok()
            && let Some(named_result) = named_result
            && !definitely_initializes_named_result(body, &named_result.name, &reach)
        {
            result = Err(TypeError::MissingReturn(name.clone()));
        }
        // A lambda is thin only when it captures nothing, writes no explicit
        // capture-all convention, and owns no parameter list; `{imm}` with
        // zero actual captures is still a closure, while an omitted list
        // (whose imm default lives only in the policy) and an explicit `{}`
        // both stay thin when nothing was captured.
        let lambda_forces_closure = lambda
            && (captures.as_ref().is_some_and(|list| list.default.is_some())
                || !type_params.is_empty());
        if result.is_ok() {
            let captures = self
                .fact(&stmt.source_span(), |facts| facts.captures.as_ref())
                .unwrap_or_default();
            let concrete = mojito_types::origin::CaptureOriginSet::concrete(
                captures
                    .iter()
                    .flat_map(|capture| capture.origins.iter().cloned()),
            );
            let environment = if parameter_closure || lambda_forces_closure || !captures.is_empty()
            {
                mojito_types::origin::CallableEnvironment::Capturing(concrete)
            } else {
                mojito_types::origin::CallableEnvironment::Thin
            };
            let finalized = with_callable_environment(fn_ty, environment);
            let function_scope = *self
                .function_bases
                .last()
                .expect("function scope remains active through finalization");
            if let Some(existing) = function_scope
                .checked_sub(1)
                .and_then(|scope| self.scopes.get_mut(scope))
                .and_then(|scope| scope.get_mut(name))
                && !matches!(existing.kind(), TyKind::Overload(_))
            {
                *existing = finalized.clone();
            }
            self.declaration_types.borrow_mut().insert(
                mojito_checked::checked::AnnotationSite::FunctionType {
                    module: stmt.module.clone(),
                    declaration: stmt.span,
                    syntax: stmt.syntax_id,
                },
                finalized,
            );
        }
        // Name the fixed-`None` rule when a lambda without `-> T` returns a
        // value, instead of the bare return mismatch.
        if lambda
            && ret_anno.is_none()
            && let Err(TypeError::TypeMismatch {
                expected,
                found,
                context,
            }) = &result
            && context == "return"
            && expected == "None"
        {
            result = Err(TypeError::TypeMismatch {
                expected: expected.clone(),
                found: found.clone(),
                context: "the lambda body (an omitted lambda return type is fixed to 'None', \
                          never inferred)"
                    .to_string(),
            });
        }
        self.pop_scope();
        self.function_bases.pop();
        if !self.function_bases.is_empty() {
            self.capture_contexts.borrow_mut().pop();
        }
        self.raising_context.pop();
        self.assumed_conformances.borrow_mut().pop();
        self.tparams.pop();
        result
    }

    /// A default value must fit its parameter's type. A default may name a
    /// compile-time value parameter (`value: Int = n`), so the value
    /// parameters are bound around the defaults as they are in the body.
    fn check_parameter_defaults(
        &mut self,
        params: &[FnParam],
        param_tys: &[Ty],
        decls: &[ParamDecl],
    ) -> Result<(), TypeError> {
        let bind_values = decls
            .iter()
            .any(|decl| matches!(decl, ParamDecl::Value { .. }));
        if !bind_values {
            return self.check_parameter_default_values(params, param_tys);
        }
        self.push_scope();
        let result = self
            .declare_value_parameters(decls)
            .and_then(|()| self.check_parameter_default_values(params, param_tys));
        self.pop_scope();
        result
    }

    fn check_parameter_default_values(
        &self,
        params: &[FnParam],
        param_tys: &[Ty],
    ) -> Result<(), TypeError> {
        for (p, pty) in params.iter().zip(param_tys) {
            let Some(d) = &p.default else {
                continue;
            };
            if let Some(name) = self.dynamic_default_reference(d) {
                return Err(TypeError::DynamicDefault(name));
            }
            let dty = self.infer_with_expected(d, pty, true)?;
            // Fall back to an `@implicit` converting constructor, like the
            // binding and argument positions do (records the ctor target so
            // the omitted-arg default materializes it). A `None` default for
            // `Optional[T]` resolves here.
            if !coerces(&dty, pty) && !self.record_constructor_conversion(d, &dty, pty)? {
                return Err(TypeError::TypeMismatch {
                    expected: pty.to_string(),
                    found: dty.to_string(),
                    context: format!("default value of '{}'", p.name),
                });
            }
        }
        Ok(())
    }

    /// The first runtime binding — an enclosing local or a parameter — a
    /// parameter default names, which current Mojo rejects: a default is
    /// evaluated at compile time.
    pub(super) fn dynamic_default_reference(&self, default: &Expr) -> Option<String> {
        struct Names(Vec<String>);
        impl mojito_ast::visit::Visitor for Names {
            fn visit_expr(&mut self, expr: &Expr) {
                if let ExprKind::Identifier(name) = &expr.kind {
                    self.0.push(name.clone());
                }
            }
        }
        let mut names = Names(Vec::new());
        mojito_ast::visit::walk_expr(&mut names, default);
        names.0.into_iter().find(|name| {
            self.binding_scope(name).is_some_and(|scope| scope > 0)
                && self.lookup(name).is_some_and(|ty| {
                    !matches!(
                        ty.kind(),
                        TyKind::Func(_) | TyKind::GenericFunc(_) | TyKind::Overload(_)
                    )
                })
                && !self.is_compile_time_binding(name)
                && !self
                    .lookup_owner(name)
                    .is_some_and(|owner| self.value_parameter_owners.contains(&owner))
                && !self
                    .local_comptime_values
                    .iter()
                    .any(|values| values.contains_key(name.as_str()))
                && !self.installs(interface::Table::ComptimeValues, name)
        })
    }

    pub(super) fn param_tys(
        &self,
        params: &[mojito_ast::ast::FnParam],
    ) -> Result<Vec<Ty>, TypeError> {
        params
            .iter()
            .map(|parameter| {
                if matches!(
                    &parameter.ty,
                    SourceType::Func { type_params, .. } if !type_params.is_empty()
                ) {
                    return Err(TypeError::Unsupported(
                        "a runtime parameter cannot use a parametric function type; declare it as a compile-time callable parameter"
                            .to_string(),
                    ));
                }
                // The initialized-local rule for parameter annotations
                // (pin-attested): bare infers per call, a partial application
                // omitting an origin slot rejects, a placeholder is inferred
                // (a bare `Pointer[T, _]` is concrete here only).
                self.resolving_parameter_annotation.set(true);
                self.bare_string_literal_parameter
                    .set(super::declarations::is_string_literal_annotation(&parameter.ty));
                let resolved = self.resolve_storage_annotation(
                    &parameter.ty,
                    super::StorageStrictness::AllowBare,
                );
                self.bare_string_literal_parameter.set(false);
                self.resolving_parameter_annotation.set(false);
                resolved
            })
            .collect()
    }

    /// Lower a generic `comptime Alias[params] = Type` declaration into the
    /// checked alias registry. The body cannot be evaluated eagerly (it
    /// references the alias's own parameters), so it is lowered once to a
    /// symbolic template and expanded per application during type resolution.
    /// The alias declares no value binding: it is a pure type declaration,
    /// like a struct or trait, with no runtime form.
    pub(super) fn check_generic_comptime_alias(
        &mut self,
        name: &str,
        type_params: &[mojito_ast::ast::TypeParam],
        ty: Option<&SourceType>,
        where_clauses: &[Expr],
        value: &Expr,
    ) -> Result<(), TypeError> {
        if !self.function_bases.is_empty() {
            return Err(TypeError::Unsupported(
                "a generic comptime alias must be declared at module scope".to_string(),
            ));
        }
        // The registry binds application arguments through classified
        // `ParamDecl`s, which erase origin parameters; reject them instead of
        // silently dropping a binder the template could still mention.
        if type_params.iter().any(|parameter| {
            matches!(parameter.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet")
                || parameter.is_origin_mutability_binder(type_params)
        }) {
            return Err(TypeError::Unsupported(
                "origin parameters on a generic comptime alias".to_string(),
            ));
        }
        if self.comptime_aliases.contains_key(name)
            || self.comptimes.contains_key(name)
            || self.structs.contains_key(name)
        {
            return Err(TypeError::Redeclaration(name.to_string()));
        }
        // The builtin `IsTrivially*` predicates are compiler spellings, not a
        // shadowable namespace: every evaluation surface recognizes them
        // before consulting the alias registry.
        if mojito_types::types::trivial_predicate_name(name).is_some() {
            return Err(TypeError::Redeclaration(name.to_string()));
        }
        // Resolve and classify the annotation, so a declared type is not
        // silently discarded (mirrors associated-member declarations): a
        // value type declares a value body, converted to it.
        let declared = match ty
            .map(|annotation| self.ct_member_req_from_anno(type_params, annotation))
            .transpose()?
        {
            Some(CtMemberReq::Value(declared)) => Some(declared),
            _ => None,
        };
        let mut decls = self.classify_params(name, type_params)?;
        for condition in where_clauses {
            let constraint = self.compile_where_clause(condition)?.bind(&[&decls]);
            let Some(last) = decls.last_mut() else {
                return Err(TypeError::Unsupported(
                    "a where clause requires compile-time parameters".to_string(),
                ));
            };
            match last {
                ParamDecl::Type { constraints, .. } | ParamDecl::Value { constraints, .. } => {
                    constraints.push(constraint);
                }
            }
        }
        // A body is a type expression, a Bool proposition (a predicate
        // alias), or a compile-time value expression. Some Bool bodies parse
        // as type applications (`IsTriviallyCopyable[T]` is an `Index` shape),
        // so a body that parses as a type but fails to lower falls back to the
        // predicate path; a genuine type error is reported when neither path
        // accepts. A body that is not a proposition is a value, lowered to
        // its template over the alias's binders, as is any body a value
        // annotation declares.
        let body = match (super::constraints::assoc_body_source_type(value), &declared) {
            // A `Bool` annotation admits a proposition as well as a value.
            (_, Some(declared)) => match self.lower_parametric_value(&decls, value, Some(declared))
            {
                Ok(body) => AliasBody::Value(body),
                Err(error) => match self.compile_predicate_alias_body(&decls, value) {
                    Ok(constraint) if *declared.kind() == TyKind::Bool => {
                        AliasBody::Predicate(Box::new(constraint))
                    }
                    _ => return Err(error),
                },
            },
            (Ok(source_ty), None) => {
                match self.lower_parameterized_member(
                    &decls,
                    type_scope(&decls),
                    type_params,
                    value,
                    &source_ty,
                ) {
                    Ok(template) => AliasBody::Type(template),
                    // A type-shaped body over a pack (`Ts.length`, `vs[0]`)
                    // may still be a proposition or a value.
                    Err(type_error) => match self.compile_predicate_alias_body(&decls, value) {
                        Ok(constraint) => AliasBody::Predicate(Box::new(constraint)),
                        Err(_) => AliasBody::Value(
                            self.lower_parametric_value(&decls, value, None)
                                .map_err(|_| type_error)?,
                        ),
                    },
                }
            }
            (Err(_), None) => match self.compile_predicate_alias_body(&decls, value) {
                Ok(constraint) => AliasBody::Predicate(Box::new(constraint)),
                Err(predicate_error) => match self.lower_parametric_value(&decls, value, None) {
                    Ok(body) => AliasBody::Value(body),
                    Err(error)
                        if matches!(&predicate_error, TypeError::Unsupported(message)
                            if message.starts_with("a generic comptime alias must be defined by")) =>
                    {
                        return Err(error);
                    }
                    Err(_) => return Err(predicate_error),
                },
            },
        };
        self.comptime_aliases
            .insert(name.to_string(), ComptimeAlias { decls, body });
        Ok(())
    }
}

const RESERVED_FUNCTION_NAMES: &[&str] = &["class", "del", "match", "yield", "case"];

/// Rejects a lambda in any of `params`' defaults: a default is inferred
/// without scoped pre-registration, so a lambda there has no declaration
/// point.
pub(super) fn reject_default_lambdas(params: &[FnParam]) -> Result<(), TypeError> {
    let mut lambdas = Vec::new();
    for default in params.iter().filter_map(|param| param.default.as_ref()) {
        mojito_ast::ast::lambdas_in_expr(default, &mut lambdas);
    }
    if lambdas.is_empty() {
        Ok(())
    } else {
        Err(TypeError::Unsupported(
            "lambda expressions in parameter default values".to_string(),
        ))
    }
}

/// Whether a value of `ty` is, or stores, a closure: a callable whose
/// environment is not `thin`, directly or as a struct argument, tuple or
/// variant element. A generic `def` that captures nothing is thin exactly as
/// a plain one is, so `(identity, offset)` stores no closure. A thin
/// callable's signature is not searched, since the value captures nothing
/// whatever contracts its parameters name.
///
/// This is the storage question alone. Whether a *named* generic or
/// overloaded callable may be assigned or returned at all is the caller's
/// rule, spelled at each site.
fn holds_closure(ty: &Ty) -> bool {
    match ty.kind() {
        TyKind::Func(_) | TyKind::GenericFunc(_) => ty.callable().is_some_and(|func| {
            !matches!(
                func.environment,
                mojito_types::origin::CallableEnvironment::Thin
            )
        }),
        TyKind::Overload(_) => true,
        TyKind::Struct(_, arguments) => arguments.iter().any(|argument| {
            matches!(argument, mojito_types::types::TyArg::Ty(element) if holds_closure(element))
        }),
        TyKind::Tuple(elements) | TyKind::RuntimePack(elements) | TyKind::Variant(elements) => {
            elements.iter().any(holds_closure)
        }
        _ => false,
    }
}

/// The source spelling of a dependent pack projection annotation
/// (`values.Ts[index]`), used as the opaque element's type name inside a
/// handler body.
fn dependent_projection_spelling(annotation: &SourceType) -> String {
    fn type_text(annotation: &SourceType) -> String {
        match annotation {
            SourceType::Named(name, _) => name.clone(),
            SourceType::Assoc { base, name, .. } => format!("{}.{name}", type_text(base)),
            SourceType::IndexedProjection { base, index } => {
                let index = match &index.kind {
                    ExprKind::Identifier(name) => name.clone(),
                    ExprKind::Int(value) => value.to_string(),
                    _ => "_".to_string(),
                };
                format!("{}[{index}]", type_text(base))
            }
            _ => "element".to_string(),
        }
    }
    type_text(annotation)
}

impl Checker {
    /// Erase one occurrence's record and its entries in the checker's own
    /// occurrence-keyed tables: the augmented store drops what it recorded
    /// at a synthesized receiver.
    fn remove_occurrence_facts(&self, span: &SourceSpan) {
        self.node_facts.borrow_mut().remove(span);
        self.construction_immutable_binders
            .borrow_mut()
            .remove(span);
        self.call_result_origins.borrow_mut().remove(span);
        self.view_result_interiors.borrow_mut().remove(span);
        self.call_parameters.borrow_mut().remove(span);
        self.statement_desugars.borrow_mut().remove(span);
        self.comprehension_iterables.borrow_mut().remove(span);
        self.nested_def_params.borrow_mut().remove(span);
        self.copyable_reference_result_reads
            .borrow_mut()
            .remove(span);
        self.discarded_reference_results.borrow_mut().remove(span);
        self.borrowed_reference_receivers.borrow_mut().remove(span);
        self.unconsumed_temporaries.borrow_mut().remove(span);
        self.linear_temporaries.borrow_mut().remove(span);
        self.raising_iterations.borrow_mut().remove(span);
        let mut deletability = self.explicit_destroy_deletability.borrow_mut();
        deletability.bindings.remove(span);
        deletability.linear_bindings.remove(span);
    }
}

/// Whether `error` is a declaration reading a struct member or constructing
/// a struct whose members or method signatures have not registered yet.
fn awaits_struct_declarations(error: &TypeError) -> bool {
    matches!(error, TypeError::NoSuchAssociatedType { .. })
        || matches!(error, TypeError::BadCall { reason, .. }
            if reason == "no '__init__' overload takes no arguments")
}

/// Whether `name` is a trait of `stmts` whose check waits in `deferred`.
fn trait_is_deferred(stmts: &[Stmt], deferred: &[usize], name: &str) -> bool {
    deferred.iter().any(|&index| {
        matches!(&stmts[index].kind, StmtKind::Trait { name: deferred_name, .. }
            if deferred_name == name)
    })
}

/// Whether `declaration` declares a conformance, unconditional or
/// conditional, to a trait whose check waits in `deferred`.
fn conforms_to_deferred(
    stmts: &[Stmt],
    deferred: &[usize],
    declaration: &StructDeclaration<'_>,
) -> bool {
    !deferred.is_empty()
        && declaration
            .conforms
            .iter()
            .chain(
                declaration
                    .conformance_conditions
                    .iter()
                    .map(|(name, _)| name),
            )
            .any(|name| trait_is_deferred(stmts, deferred, name))
}

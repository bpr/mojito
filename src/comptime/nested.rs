//! Lexically scoped specialization of generic nested functions.
//!
//! Top-level specialization deliberately runs first. Each concrete outer
//! function then owns a small local registry whose declaration IDs distinguish
//! same-spelled helpers in different functions, blocks, and outer
//! specializations. Templates stay at their source declaration site and are
//! replaced there by only the concrete instances requested in that lexical
//! context.

use super::*;
use crate::ast::ParamKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct NestedTemplateId(u32);

#[derive(Debug, Clone, Copy)]
enum TemplateBinding {
    Other,
    Template(NestedTemplateId),
}

#[derive(Clone)]
struct NestedTemplate {
    source_name: String,
    marker_name: String,
    syntax: Stmt,
    outer_packs: HashMap<String, Vec<Type>>,
}

struct NestedJob {
    template: NestedTemplateId,
    values: Vec<CtValue>,
    output_name: String,
    site: String,
    whole_pack_abi: bool,
}

#[derive(Clone)]
enum RuntimePackBinding {
    Other,
    Pack(Vec<Type>),
}

#[derive(Clone)]
struct RuntimePackEnv {
    scopes: Vec<HashMap<String, RuntimePackBinding>>,
    function_scopes: Vec<usize>,
}

impl RuntimePackEnv {
    fn new(packs: HashMap<String, Vec<Type>>) -> Self {
        Self {
            scopes: vec![
                packs
                    .into_iter()
                    .map(|(name, types)| (name, RuntimePackBinding::Pack(types)))
                    .collect(),
            ],
            function_scopes: vec![0],
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn push_function_scope(&mut self) {
        self.push_scope();
        self.function_scopes.push(self.scopes.len() - 1);
    }

    fn pop_function_scope(&mut self) {
        self.function_scopes.pop();
        self.pop_scope();
    }

    fn bind_other(&mut self, name: &str) {
        self.scopes
            .last_mut()
            .expect("runtime-pack lookup always has a lexical scope")
            .insert(name.to_string(), RuntimePackBinding::Other);
    }

    fn bind_pack(&mut self, name: String, types: Vec<Type>) {
        self.scopes
            .last_mut()
            .expect("runtime-pack lookup always has a lexical scope")
            .insert(name, RuntimePackBinding::Pack(types));
    }

    fn bind_named(&mut self, name: &str) {
        let base = *self
            .function_scopes
            .last()
            .expect("runtime-pack lookup always runs inside a function");
        if let Some(scope) = self.scopes[base..]
            .iter_mut()
            .rev()
            .find(|scope| scope.contains_key(name))
        {
            scope.insert(name.to_string(), RuntimePackBinding::Other);
        } else {
            self.scopes[base].insert(name.to_string(), RuntimePackBinding::Other);
        }
    }

    fn resolve_pack(&self, name: &str) -> Option<&[Type]> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .and_then(|binding| match binding {
                RuntimePackBinding::Pack(types) => Some(types.as_slice()),
                RuntimePackBinding::Other => None,
            })
    }

    fn visible_packs(&self) -> HashMap<String, Vec<Type>> {
        let mut visible = HashMap::new();
        for scope in &self.scopes {
            for (name, binding) in scope {
                match binding {
                    RuntimePackBinding::Pack(types) => {
                        visible.insert(name.clone(), types.clone());
                    }
                    RuntimePackBinding::Other => {
                        visible.remove(name);
                    }
                }
            }
        }
        visible
    }
}

struct NestedMono {
    parent: String,
    next_template: u32,
    scopes: Vec<HashMap<String, TemplateBinding>>,
    function_scopes: Vec<usize>,
    templates: HashMap<NestedTemplateId, NestedTemplate>,
    markers: HashMap<String, NestedTemplateId>,
    queue: VecDeque<NestedJob>,
    done: HashSet<String>,
    generated: HashMap<NestedTemplateId, Vec<Stmt>>,
}

impl NestedMono {
    fn new(parent: String, parameters: &[crate::ast::FnParam], has_self: bool) -> Self {
        let mut root = HashMap::new();
        for parameter in parameters {
            root.insert(parameter.name.clone(), TemplateBinding::Other);
        }
        if has_self {
            root.insert("self".to_string(), TemplateBinding::Other);
        }
        Self {
            parent,
            next_template: 0,
            scopes: vec![root],
            function_scopes: vec![0],
            templates: HashMap::new(),
            markers: HashMap::new(),
            queue: VecDeque::new(),
            done: HashSet::new(),
            generated: HashMap::new(),
        }
    }

    fn fresh_template(&mut self) -> NestedTemplateId {
        let id = NestedTemplateId(self.next_template);
        self.next_template += 1;
        id
    }

    fn marker(&self, id: NestedTemplateId, source_name: &str) -> String {
        // `$` cannot occur in a parsed identifier. The concrete parent name
        // already contains any outer-specialization encoding, so this is also
        // the enclosing specialization environment's canonical identity.
        format!("{}$nested${}${source_name}", self.parent, id.0)
    }

    fn bind(&mut self, name: &str, binding: TemplateBinding) {
        self.scopes
            .last_mut()
            .expect("nested specialization always has a lexical scope")
            .insert(name.to_string(), binding);
    }

    fn resolve(&self, name: &str) -> Option<TemplateBinding> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn push_function_scope(&mut self) {
        self.push_scope();
        self.function_scopes.push(self.scopes.len() - 1);
    }

    fn pop_function_scope(&mut self) {
        self.function_scopes.pop();
        self.pop_scope();
    }

    fn bind_named(&mut self, name: &str) {
        let base = *self
            .function_scopes
            .last()
            .expect("nested specialization always runs inside a function");
        if let Some(scope) = self.scopes[base..]
            .iter_mut()
            .rev()
            .find(|scope| scope.contains_key(name))
        {
            scope.insert(name.to_string(), TemplateBinding::Other);
        } else {
            self.scopes[base].insert(name.to_string(), TemplateBinding::Other);
        }
    }

    fn qualify_root(&mut self, statements: &mut [Stmt]) {
        self.qualify_block_contents(statements, 0);
    }

    fn qualify_block(&mut self, statements: &mut [Stmt], definition_depth: usize) {
        self.push_scope();
        self.qualify_block_contents(statements, definition_depth);
        self.pop_scope();
    }

    fn qualify_block_contents(&mut self, statements: &mut [Stmt], definition_depth: usize) {
        for statement in statements {
            self.qualify_statement(statement, definition_depth);
        }
    }

    fn qualify_statement(&mut self, statement: &mut Stmt, definition_depth: usize) {
        if matches!(statement.kind, StmtKind::Def { .. }) {
            self.qualify_definition(statement, definition_depth);
            return;
        }
        match &mut statement.kind {
            StmtKind::VarDecl { name, ty, value } => {
                if let Some(ty) = ty {
                    self.qualify_type(ty);
                }
                self.qualify_expression(value);
                self.bind(name, TemplateBinding::Other);
            }
            StmtKind::RefDecl { name, value } => {
                self.qualify_expression(value);
                self.bind(name, TemplateBinding::Other);
            }
            StmtKind::Assign { value, .. } | StmtKind::Comptime { value, .. } => {
                self.qualify_expression(value)
            }
            StmtKind::AugAssign { place, value, .. } | StmtKind::SetPlace { place, value } => {
                self.qualify_expression(place);
                self.qualify_expression(value);
            }
            StmtKind::Unpack { targets, value } => {
                for target in targets {
                    self.qualify_expression(target);
                }
                self.qualify_expression(value);
            }
            StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
                for (condition, body) in branches {
                    self.qualify_expression(condition);
                    self.qualify_block(body, definition_depth);
                }
                if let Some(body) = orelse {
                    self.qualify_block(body, definition_depth);
                }
            }
            StmtKind::While { cond, body, orelse } => {
                self.qualify_expression(cond);
                self.qualify_block(body, definition_depth);
                if let Some(body) = orelse {
                    self.qualify_block(body, definition_depth);
                }
            }
            StmtKind::For {
                var,
                iter,
                body,
                orelse,
                ..
            } => {
                self.qualify_expression(iter);
                self.push_scope();
                self.bind(var, TemplateBinding::Other);
                self.qualify_block_contents(body, definition_depth);
                self.pop_scope();
                if let Some(body) = orelse {
                    self.qualify_block(body, definition_depth);
                }
            }
            StmtKind::ComptimeFor { var, iter, body } => {
                self.qualify_expression(iter);
                self.push_scope();
                self.bind(var, TemplateBinding::Other);
                self.qualify_block_contents(body, definition_depth);
                self.pop_scope();
            }
            StmtKind::Return(value) => {
                if let Some(value) = value {
                    self.qualify_expression(value);
                }
            }
            StmtKind::Raise(value) | StmtKind::Expr(value) => self.qualify_expression(value),
            StmtKind::With { items, body } => {
                for item in items.iter_mut() {
                    self.qualify_expression(&mut item.context);
                }
                self.push_scope();
                for item in items {
                    if let Some(name) = &item.var {
                        self.bind(name, TemplateBinding::Other);
                    }
                }
                self.qualify_block_contents(body, definition_depth);
                self.pop_scope();
            }
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                self.qualify_block(body, definition_depth);
                if let Some((name, body)) = except {
                    self.push_scope();
                    if let Some(name) = name {
                        self.bind(name, TemplateBinding::Other);
                    }
                    self.qualify_block_contents(body, definition_depth);
                    self.pop_scope();
                }
                if let Some(body) = orelse {
                    self.qualify_block(body, definition_depth);
                }
                if let Some(body) = finalbody {
                    self.qualify_block(body, definition_depth);
                }
            }
            StmtKind::Struct { name, .. } | StmtKind::Trait { name, .. } => {
                self.bind(name, TemplateBinding::Other)
            }
            StmtKind::Import { path, alias } => {
                if let Some(name) = alias.as_ref().or_else(|| path.first()) {
                    self.bind(name, TemplateBinding::Other);
                }
            }
            StmtKind::FromImport { names, .. } => {
                if let crate::ast::ImportNames::Names(names) = names {
                    for import in names {
                        self.bind(
                            import.alias.as_deref().unwrap_or(&import.name),
                            TemplateBinding::Other,
                        );
                    }
                }
            }
            StmtKind::Pass | StmtKind::Break | StmtKind::Continue => {}
            StmtKind::Def { .. } => unreachable!("definitions are handled above"),
        }
    }

    fn qualify_definition(&mut self, statement: &mut Stmt, definition_depth: usize) {
        let specializable = definition_depth == 0 && is_specializable_declaration(statement);
        let (source_name, id, marker_name) = {
            let StmtKind::Def {
                name,
                decorators,
                type_params,
                params,
                raises_type,
                ret,
                where_clause,
                body,
                ..
            } = &mut statement.kind
            else {
                unreachable!()
            };

            for decorator in decorators {
                for argument in &mut decorator.args {
                    self.qualify_expression(argument);
                }
                for argument in &mut decorator.kwargs {
                    self.qualify_expression(&mut argument.value);
                }
            }
            for parameter in type_params.iter_mut() {
                self.qualify_type_parameter(parameter);
            }
            for parameter in params.iter_mut() {
                self.qualify_type(&mut parameter.ty);
                if let Some(default) = &mut parameter.default {
                    self.qualify_expression(default);
                }
            }
            if let Some(error) = raises_type {
                self.qualify_type(error);
            }
            if let Some(ret) = ret {
                self.qualify_type(ret);
            }
            if let Some(predicate) = where_clause {
                self.qualify_expression(predicate);
            }

            let source_name = name.clone();
            let (id, marker_name) = if specializable {
                let id = self.fresh_template();
                let marker = self.marker(id, &source_name);
                self.bind(&source_name, TemplateBinding::Template(id));
                *name = marker.clone();
                (Some(id), Some(marker))
            } else {
                self.bind(&source_name, TemplateBinding::Other);
                (None, None)
            };

            self.push_function_scope();
            for parameter in params {
                self.bind(&parameter.name, TemplateBinding::Other);
            }
            self.qualify_block_contents(body, definition_depth + 1);
            self.pop_function_scope();
            (source_name, id, marker_name)
        };

        if let (Some(id), Some(marker_name)) = (id, marker_name) {
            let template = NestedTemplate {
                source_name,
                marker_name: marker_name.clone(),
                syntax: statement.clone(),
                outer_packs: HashMap::new(),
            };
            self.markers.insert(marker_name, id);
            self.templates.insert(id, template);
        }
    }

    fn qualify_type(&mut self, ty: &mut Type) {
        match ty {
            Type::Named(_, arguments) => {
                for argument in arguments {
                    self.qualify_param_arg(argument);
                }
            }
            Type::Assoc { base, .. } => self.qualify_type(base),
            Type::IndexedProjection { base, index } => {
                self.qualify_type(base);
                self.qualify_expression(index);
            }
            Type::Func {
                type_params,
                params,
                ret,
                capturing,
                raises_type,
                ..
            } => {
                for parameter in type_params {
                    self.qualify_type_parameter(parameter);
                }
                for parameter in params {
                    self.qualify_type(&mut parameter.ty);
                    for expression in parameter.origin.iter_mut().flatten() {
                        self.qualify_expression(expression);
                    }
                }
                self.qualify_type(ret);
                for expression in capturing.iter_mut().flatten() {
                    self.qualify_expression(expression);
                }
                if let Some(error) = raises_type {
                    self.qualify_type(error);
                }
            }
            Type::Ref { referent, origin } => {
                self.qualify_type(referent);
                for expression in origin.iter_mut().flatten() {
                    self.qualify_expression(expression);
                }
            }
            Type::Int
            | Type::UInt
            | Type::Bool
            | Type::String
            | Type::Float64
            | Type::None
            | Type::SelfParam(_)
            | Type::SelfType
            | Type::MaterializedCallable(_) => {}
        }
    }

    fn qualify_type_parameter(&mut self, parameter: &mut crate::ast::TypeParam) {
        if let Some(value_type) = &mut parameter.value_type {
            self.qualify_type(value_type);
        }
        if let Some(callable) = &mut parameter.callable_bound {
            self.qualify_type(callable);
        }
        if let Some(mutability) = &mut parameter.origin_mutability {
            self.qualify_expression(mutability);
        }
        if let Some(default) = &mut parameter.default {
            self.qualify_expression(default);
        }
        for constraint in &mut parameter.constraints {
            self.qualify_expression(constraint);
        }
    }

    fn qualify_param_arg(&mut self, argument: &mut ParamArg) {
        match argument {
            ParamArg::Type(ty) => self.qualify_type(ty),
            ParamArg::Value(value) => self.qualify_expression(value),
            ParamArg::Named { value, .. } => self.qualify_param_arg(value),
        }
    }

    fn qualify_expression(&mut self, expression: &mut Expr) {
        match &mut expression.kind {
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } => {
                if let Some(TemplateBinding::Template(id)) = self.resolve(name)
                    && let Some(template) = self.templates.get(&id)
                {
                    *name = template.marker_name.clone();
                } else if let Some(TemplateBinding::Template(id)) = self.resolve(name) {
                    // Self references are qualified before the declaration clone
                    // has been entered into `templates`.
                    *name = self.marker(id, name);
                }
                for argument in param_args {
                    self.qualify_param_arg(argument);
                }
                for argument in args {
                    self.qualify_expression(argument);
                }
                for argument in kwargs {
                    self.qualify_expression(&mut argument.value);
                }
            }
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } => {
                self.qualify_expression(callee);
                for argument in param_args {
                    self.qualify_param_arg(argument);
                }
                for argument in args {
                    self.qualify_expression(argument);
                }
                for argument in kwargs {
                    self.qualify_expression(&mut argument.value);
                }
            }
            ExprKind::TypeApply { args, .. } => {
                for argument in args {
                    self.qualify_param_arg(argument);
                }
            }
            ExprKind::Prefix(_, value) | ExprKind::Transfer(value) | ExprKind::Spread(value) => {
                self.qualify_expression(value)
            }
            ExprKind::Infix(_, left, right)
            | ExprKind::Index {
                object: left,
                index: right,
            } => {
                self.qualify_expression(left);
                self.qualify_expression(right);
            }
            ExprKind::Compare { first, rest } => {
                self.qualify_expression(first);
                for (_, value) in rest {
                    self.qualify_expression(value);
                }
            }
            ExprKind::Member { object, .. } => self.qualify_expression(object),
            ExprKind::MethodCall {
                object,
                args,
                kwargs,
                ..
            } => {
                self.qualify_expression(object);
                for argument in args {
                    self.qualify_expression(argument);
                }
                for argument in kwargs {
                    self.qualify_expression(&mut argument.value);
                }
            }
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                ..
            } => {
                self.qualify_expression(object);
                for value in [lower, upper, step].into_iter().flatten() {
                    self.qualify_expression(value);
                }
            }
            ExprKind::MultiIndex { object, args } => {
                self.qualify_expression(object);
                for argument in args {
                    match argument {
                        crate::ast::SubscriptArg::Index(value) => self.qualify_expression(value),
                        crate::ast::SubscriptArg::Slice {
                            lower, upper, step, ..
                        } => {
                            for value in [lower, upper, step].into_iter().flatten() {
                                self.qualify_expression(value);
                            }
                        }
                    }
                }
            }
            ExprKind::ListLit(values) | ExprKind::TupleLit(values) => {
                for value in values {
                    self.qualify_expression(value);
                }
            }
            ExprKind::BraceLit(entries) => {
                for (key, value) in entries {
                    self.qualify_expression(key);
                    if let Some(value) = value {
                        self.qualify_expression(value);
                    }
                }
            }
            ExprKind::Comprehension {
                key,
                value,
                clauses,
                ..
            } => {
                self.push_scope();
                for clause in clauses {
                    match clause {
                        crate::ast::ComprehensionClause::For { var, iter, .. } => {
                            self.qualify_expression(iter);
                            self.bind(var, TemplateBinding::Other);
                        }
                        crate::ast::ComprehensionClause::If(condition) => {
                            self.qualify_expression(condition)
                        }
                    }
                }
                if let Some(key) = key {
                    self.qualify_expression(key);
                }
                self.qualify_expression(value);
                self.pop_scope();
            }
            ExprKind::TypeValue(ty) => self.qualify_type(ty),
            ExprKind::Named { name, value } => {
                self.qualify_expression(value);
                self.bind_named(name);
            }
            ExprKind::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => {
                self.qualify_expression(cond);
                self.qualify_expression(then_branch);
                self.qualify_expression(else_branch);
            }
            ExprKind::TString { parts, .. } => {
                for part in parts {
                    if let crate::ast::TStringPart::Expr(value) = part {
                        self.qualify_expression(value);
                    }
                }
            }
            ExprKind::Identifier(name) => {
                if let Some(TemplateBinding::Template(id)) = self.resolve(name) {
                    *name = self
                        .templates
                        .get(&id)
                        .map(|template| template.marker_name.clone())
                        .unwrap_or_else(|| self.marker(id, name));
                }
            }
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::None
            | ExprKind::Uninitialized => {}
        }
    }

    fn scan_root(
        &mut self,
        elab: &Elab<'_>,
        statements: &mut [Stmt],
        runtime_packs: &mut RuntimePackEnv,
    ) -> Result<(), ComptimeError> {
        for statement in statements {
            self.scan_statement(elab, statement, runtime_packs)?;
        }
        Ok(())
    }

    fn scan_block(
        &mut self,
        elab: &Elab<'_>,
        statements: &mut [Stmt],
        runtime_packs: &mut RuntimePackEnv,
    ) -> Result<(), ComptimeError> {
        runtime_packs.push_scope();
        let result = self.scan_root(elab, statements, runtime_packs);
        runtime_packs.pop_scope();
        result
    }

    fn scan_statement(
        &mut self,
        elab: &Elab<'_>,
        statement: &mut Stmt,
        runtime_packs: &mut RuntimePackEnv,
    ) -> Result<(), ComptimeError> {
        if let StmtKind::Def { name, .. } = &statement.kind
            && let Some(&template) = self.markers.get(name)
        {
            self.templates
                .get_mut(&template)
                .expect("nested marker has a registered template")
                .outer_packs = runtime_packs.visible_packs();
            runtime_packs.bind_other(name);
            return Ok(()); // deferred template; scan only concrete instances
        }
        match &mut statement.kind {
            StmtKind::VarDecl { name, value, .. }
            | StmtKind::RefDecl { name, value }
            | StmtKind::Comptime { name, value } => {
                self.scan_expression(elab, value, runtime_packs)?;
                runtime_packs.bind_other(name);
                Ok(())
            }
            StmtKind::Assign { value, .. } | StmtKind::Raise(value) | StmtKind::Expr(value) => {
                self.scan_expression(elab, value, runtime_packs)
            }
            StmtKind::AugAssign { place, value, .. } | StmtKind::SetPlace { place, value } => {
                self.scan_expression(elab, place, runtime_packs)?;
                self.scan_expression(elab, value, runtime_packs)
            }
            StmtKind::Unpack { targets, value } => {
                self.scan_expression(elab, value, runtime_packs)?;
                for target in targets {
                    self.scan_expression(elab, target, runtime_packs)?;
                    if let ExprKind::Identifier(name) = &target.kind {
                        runtime_packs.bind_other(name);
                    }
                }
                Ok(())
            }
            StmtKind::Return(value) => {
                if let Some(value) = value {
                    self.scan_expression(elab, value, runtime_packs)?;
                }
                Ok(())
            }
            StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
                for (condition, body) in branches {
                    self.scan_expression(elab, condition, runtime_packs)?;
                    self.scan_block(elab, body, runtime_packs)?;
                }
                if let Some(body) = orelse {
                    self.scan_block(elab, body, runtime_packs)?;
                }
                Ok(())
            }
            StmtKind::While { cond, body, orelse } => {
                self.scan_expression(elab, cond, runtime_packs)?;
                self.scan_block(elab, body, runtime_packs)?;
                if let Some(body) = orelse {
                    self.scan_block(elab, body, runtime_packs)?;
                }
                Ok(())
            }
            StmtKind::For {
                var,
                iter,
                body,
                orelse,
                ..
            } => {
                self.scan_expression(elab, iter, runtime_packs)?;
                runtime_packs.push_scope();
                runtime_packs.bind_other(var);
                self.scan_root(elab, body, runtime_packs)?;
                runtime_packs.pop_scope();
                if let Some(body) = orelse {
                    self.scan_block(elab, body, runtime_packs)?;
                }
                Ok(())
            }
            StmtKind::ComptimeFor { var, iter, body } => {
                self.scan_expression(elab, iter, runtime_packs)?;
                runtime_packs.push_scope();
                runtime_packs.bind_other(var);
                let result = self.scan_root(elab, body, runtime_packs);
                runtime_packs.pop_scope();
                result
            }
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                self.scan_block(elab, body, runtime_packs)?;
                if let Some((name, body)) = except {
                    runtime_packs.push_scope();
                    if let Some(name) = name {
                        runtime_packs.bind_other(name);
                    }
                    self.scan_root(elab, body, runtime_packs)?;
                    runtime_packs.pop_scope();
                }
                if let Some(body) = orelse {
                    self.scan_block(elab, body, runtime_packs)?;
                }
                if let Some(body) = finalbody {
                    self.scan_block(elab, body, runtime_packs)?;
                }
                Ok(())
            }
            StmtKind::With { items, body } => {
                for item in items.iter_mut() {
                    self.scan_expression(elab, &mut item.context, runtime_packs)?;
                }
                runtime_packs.push_scope();
                for item in items {
                    if let Some(name) = &item.var {
                        runtime_packs.bind_other(name);
                    }
                }
                let result = self.scan_root(elab, body, runtime_packs);
                runtime_packs.pop_scope();
                result
            }
            StmtKind::Def {
                name, params, body, ..
            } => {
                for parameter in params.iter_mut() {
                    if let Some(default) = &mut parameter.default {
                        self.scan_expression(elab, default, runtime_packs)?;
                    }
                }
                runtime_packs.bind_other(name);
                runtime_packs.push_function_scope();
                for parameter in params.iter() {
                    runtime_packs.bind_other(&parameter.name);
                }
                for (name, types) in runtime_pack_types(params) {
                    runtime_packs.bind_pack(name, types);
                }
                let result = self.scan_root(elab, body, runtime_packs);
                runtime_packs.pop_function_scope();
                result
            }
            StmtKind::Struct { name, methods, .. } => {
                runtime_packs.bind_other(name);
                for method in methods {
                    runtime_packs.push_function_scope();
                    if method.has_self {
                        runtime_packs.bind_other("self");
                    }
                    for parameter in &method.params {
                        runtime_packs.bind_other(&parameter.name);
                    }
                    for (name, types) in runtime_pack_types(&method.params) {
                        runtime_packs.bind_pack(name, types);
                    }
                    self.scan_root(elab, &mut method.body, runtime_packs)?;
                    runtime_packs.pop_function_scope();
                }
                Ok(())
            }
            StmtKind::Pass | StmtKind::Break | StmtKind::Continue => Ok(()),
            StmtKind::Import { path, alias } => {
                if let Some(name) = alias.as_ref().or_else(|| path.first()) {
                    runtime_packs.bind_other(name);
                }
                Ok(())
            }
            StmtKind::FromImport { names, .. } => {
                if let crate::ast::ImportNames::Names(names) = names {
                    for import in names {
                        runtime_packs.bind_other(import.alias.as_deref().unwrap_or(&import.name));
                    }
                }
                Ok(())
            }
            StmtKind::Trait { name, .. } => {
                runtime_packs.bind_other(name);
                Ok(())
            }
        }
    }

    fn scan_expression(
        &mut self,
        elab: &Elab<'_>,
        expression: &mut Expr,
        runtime_packs: &mut RuntimePackEnv,
    ) -> Result<(), ComptimeError> {
        let source_span = expression.source_span();
        match &mut expression.kind {
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } => {
                for argument in param_args.iter_mut() {
                    self.scan_param_arg(elab, argument, runtime_packs)?;
                }
                for argument in args.iter_mut() {
                    self.scan_expression(elab, argument, runtime_packs)?;
                }
                for argument in kwargs.iter_mut() {
                    self.scan_expression(elab, &mut argument.value, runtime_packs)?;
                }
                let Some(&template_id) = self.markers.get(name) else {
                    return Ok(());
                };
                let template = self.templates[&template_id].clone();
                let site = match source_span.source {
                    Some(source) => {
                        format!("{source}:{}..{}", source_span.span.0, source_span.span.1)
                    }
                    None => format!("bytes {}..{}", source_span.span.0, source_span.span.1),
                };
                let forwarded = forwarded_pack_types(
                    &template.syntax,
                    &template.source_name,
                    args,
                    kwargs,
                    runtime_packs,
                )?;
                let (values, kept_type_args) = elab.resolve_spec_args_for(
                    &template.syntax,
                    &template.source_name,
                    SpecRequest {
                        param_args,
                        call_args: args,
                        kwargs,
                        consts: &elab.top_consts.borrow(),
                        request_site: &site,
                        forwarded_pack_types: forwarded.as_deref(),
                    },
                )?;
                // Current Mojo permits one whole runtime-pack segment after a
                // fully supplied fixed positional prefix.  Preserve that
                // segment as the Tuple collector the caller already owns: an
                // element-wise rewrite would manufacture independently movable
                // indexed storage and is unsound for linear values.  Mojo does
                // not currently permit concatenating multiple unpacked
                // positional segments or mixing explicit overflow values with
                // one, so reject those forms instead of extending the language.
                let whole_pack_abi = whole_pack_forwarding_call(&template.syntax, args)?;
                if whole_pack_abi {
                    *args = unwrap_forwarded_pack_arguments(std::mem::take(args));
                }
                let mut output_name = mangle(&template.marker_name, &values);
                if whole_pack_abi {
                    output_name.push_str("$whole_pack");
                }
                if self.done.insert(output_name.clone()) {
                    self.queue.push_back(NestedJob {
                        template: template_id,
                        values,
                        output_name: output_name.clone(),
                        site,
                        whole_pack_abi,
                    });
                }
                *name = output_name;
                *param_args = kept_type_args;
                Ok(())
            }
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } => {
                self.scan_expression(elab, callee, runtime_packs)?;
                for argument in param_args {
                    self.scan_param_arg(elab, argument, runtime_packs)?;
                }
                for argument in args {
                    self.scan_expression(elab, argument, runtime_packs)?;
                }
                for argument in kwargs {
                    self.scan_expression(elab, &mut argument.value, runtime_packs)?;
                }
                Ok(())
            }
            ExprKind::TypeApply { args, .. } => {
                for argument in args {
                    self.scan_param_arg(elab, argument, runtime_packs)?;
                }
                Ok(())
            }
            ExprKind::Prefix(_, value) | ExprKind::Transfer(value) | ExprKind::Spread(value) => {
                self.scan_expression(elab, value, runtime_packs)
            }
            ExprKind::Infix(_, left, right)
            | ExprKind::Index {
                object: left,
                index: right,
            } => {
                self.scan_expression(elab, left, runtime_packs)?;
                self.scan_expression(elab, right, runtime_packs)
            }
            ExprKind::Compare { first, rest } => {
                self.scan_expression(elab, first, runtime_packs)?;
                for (_, value) in rest {
                    self.scan_expression(elab, value, runtime_packs)?;
                }
                Ok(())
            }
            ExprKind::Member { object, .. } => self.scan_expression(elab, object, runtime_packs),
            ExprKind::MethodCall {
                object,
                args,
                kwargs,
                ..
            } => {
                self.scan_expression(elab, object, runtime_packs)?;
                for argument in args {
                    self.scan_expression(elab, argument, runtime_packs)?;
                }
                for argument in kwargs {
                    self.scan_expression(elab, &mut argument.value, runtime_packs)?;
                }
                Ok(())
            }
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                ..
            } => {
                self.scan_expression(elab, object, runtime_packs)?;
                for value in [lower, upper, step].into_iter().flatten() {
                    self.scan_expression(elab, value, runtime_packs)?;
                }
                Ok(())
            }
            ExprKind::MultiIndex { object, args } => {
                self.scan_expression(elab, object, runtime_packs)?;
                for argument in args {
                    match argument {
                        crate::ast::SubscriptArg::Index(value) => {
                            self.scan_expression(elab, value, runtime_packs)?
                        }
                        crate::ast::SubscriptArg::Slice {
                            lower, upper, step, ..
                        } => {
                            for value in [lower, upper, step].into_iter().flatten() {
                                self.scan_expression(elab, value, runtime_packs)?;
                            }
                        }
                    }
                }
                Ok(())
            }
            ExprKind::ListLit(values) | ExprKind::TupleLit(values) => {
                for value in values {
                    self.scan_expression(elab, value, runtime_packs)?;
                }
                Ok(())
            }
            ExprKind::BraceLit(entries) => {
                for (key, value) in entries {
                    self.scan_expression(elab, key, runtime_packs)?;
                    if let Some(value) = value {
                        self.scan_expression(elab, value, runtime_packs)?;
                    }
                }
                Ok(())
            }
            ExprKind::Comprehension {
                key,
                value,
                clauses,
                ..
            } => {
                runtime_packs.push_scope();
                for clause in clauses {
                    match clause {
                        crate::ast::ComprehensionClause::For { var, iter, .. } => {
                            self.scan_expression(elab, iter, runtime_packs)?;
                            runtime_packs.bind_other(var);
                        }
                        crate::ast::ComprehensionClause::If(condition) => {
                            self.scan_expression(elab, condition, runtime_packs)?
                        }
                    }
                }
                if let Some(key) = key {
                    self.scan_expression(elab, key, runtime_packs)?;
                }
                let result = self.scan_expression(elab, value, runtime_packs);
                runtime_packs.pop_scope();
                result
            }
            ExprKind::Named { name, value } => {
                self.scan_expression(elab, value, runtime_packs)?;
                runtime_packs.bind_named(name);
                Ok(())
            }
            ExprKind::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => {
                self.scan_expression(elab, cond, runtime_packs)?;
                self.scan_expression(elab, then_branch, runtime_packs)?;
                self.scan_expression(elab, else_branch, runtime_packs)
            }
            ExprKind::TString { parts, .. } => {
                for part in parts {
                    if let crate::ast::TStringPart::Expr(value) = part {
                        self.scan_expression(elab, value, runtime_packs)?;
                    }
                }
                Ok(())
            }
            ExprKind::TypeValue(_)
            | ExprKind::Identifier(_)
            | ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::None
            | ExprKind::Uninitialized => Ok(()),
        }
    }

    fn scan_param_arg(
        &mut self,
        elab: &Elab<'_>,
        argument: &mut ParamArg,
        runtime_packs: &mut RuntimePackEnv,
    ) -> Result<(), ComptimeError> {
        match argument {
            ParamArg::Value(value) => self.scan_expression(elab, value, runtime_packs),
            ParamArg::Named { value, .. } => self.scan_param_arg(elab, value, runtime_packs),
            ParamArg::Type(_) => Ok(()),
        }
    }

    fn drain(&mut self, elab: &Elab<'_>) -> Result<(), ComptimeError> {
        while let Some(job) = self.queue.pop_front() {
            elab.burn().map_err(|_| {
                ComptimeError::NotComptime(format!(
                    "specialization quota exceeded while instantiating '{}' requested at {}; possible unbounded nested generic recursion",
                    job.output_name, job.site
                ))
            })?;
            let template = self.templates[&job.template].clone();
            let mut specialization = elab.generate_def_spec(
                &template.syntax,
                &template.source_name,
                job.output_name,
                &job.values,
            )?;
            let StmtKind::Def { params, .. } = &specialization.kind else {
                unreachable!("nested templates are functions")
            };
            // Retain the logical pack facts while selecting a whole-Tuple call
            // ABI. A specialization may itself forward the collector onward.
            let packs = runtime_pack_types(params);
            let mut pack_environment = RuntimePackEnv::new(template.outer_packs.clone());
            for parameter in params {
                pack_environment.bind_other(&parameter.name);
            }
            for (name, types) in packs {
                pack_environment.bind_pack(name, types);
            }
            if job.whole_pack_abi {
                select_whole_pack_abi(&mut specialization)?;
            }
            let StmtKind::Def { body, .. } = &mut specialization.kind else {
                unreachable!("nested templates are functions")
            };
            self.scan_root(elab, body, &mut pack_environment)?;
            self.generated
                .entry(job.template)
                .or_default()
                .push(specialization);
        }
        Ok(())
    }

    fn replace_templates(&mut self, statements: &mut Vec<Stmt>) {
        let mut output = Vec::with_capacity(statements.len());
        for mut statement in std::mem::take(statements) {
            let marker = match &statement.kind {
                StmtKind::Def { name, .. } => self.markers.get(name).copied(),
                _ => None,
            };
            if let Some(template) = marker {
                if let Some(mut generated) = self.generated.remove(&template) {
                    generated.reverse();
                    output.extend(generated);
                }
                continue; // unused templates are dead and disappear
            }
            self.replace_in_statement(&mut statement);
            output.push(statement);
        }
        *statements = output;
    }

    fn replace_in_statement(&mut self, statement: &mut Stmt) {
        match &mut statement.kind {
            StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
                for (_, body) in branches {
                    self.replace_templates(body);
                }
                if let Some(body) = orelse {
                    self.replace_templates(body);
                }
            }
            StmtKind::While { body, orelse, .. } | StmtKind::For { body, orelse, .. } => {
                self.replace_templates(body);
                if let Some(body) = orelse {
                    self.replace_templates(body);
                }
            }
            StmtKind::ComptimeFor { body, .. } | StmtKind::With { body, .. } => {
                self.replace_templates(body)
            }
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                self.replace_templates(body);
                if let Some((_, body)) = except {
                    self.replace_templates(body);
                }
                if let Some(body) = orelse {
                    self.replace_templates(body);
                }
                if let Some(body) = finalbody {
                    self.replace_templates(body);
                }
            }
            StmtKind::Def { body, .. } => self.replace_templates(body),
            StmtKind::Struct { methods, .. } => {
                for method in methods {
                    self.replace_templates(&mut method.body);
                }
            }
            _ => {}
        }
    }
}

fn runtime_pack_types(parameters: &[crate::ast::FnParam]) -> HashMap<String, Vec<Type>> {
    parameters
        .iter()
        .filter_map(|parameter| {
            let Type::Named(name, arguments) = &parameter.ty else {
                return None;
            };
            if name != "$pack" {
                return None;
            }
            let types = arguments
                .iter()
                .map(|argument| match argument {
                    ParamArg::Type(ty) => Some(ty.clone()),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?;
            Some((parameter.name.clone(), types))
        })
        .collect()
}

fn spread_source(expression: &Expr) -> Option<(&str, bool)> {
    let ExprKind::Spread(value) = &expression.kind else {
        return None;
    };
    match &value.kind {
        ExprKind::Identifier(name) => Some((name, false)),
        ExprKind::Transfer(value) => match &value.kind {
            ExprKind::Identifier(name) => Some((name, true)),
            _ => None,
        },
        _ => None,
    }
}

fn whole_pack_forwarding_call(template: &Stmt, arguments: &[Expr]) -> Result<bool, ComptimeError> {
    let spread_indices = arguments
        .iter()
        .enumerate()
        .filter_map(|(index, argument)| spread_source(argument).is_some().then_some(index))
        .collect::<Vec<_>>();
    if spread_indices.is_empty() {
        return Ok(false);
    }
    if spread_indices.len() != 1 {
        return Err(ComptimeError::NotComptime(
            "concatenating unpacked positional arguments is not supported; a call may contain at most one runtime-pack spread"
                .to_string(),
        ));
    }
    let StmtKind::Def { params, .. } = &template.kind else {
        unreachable!("nested specialization templates are functions")
    };
    let Some(pack_index) = params
        .iter()
        .position(|parameter| parameter.kind == ParamKind::Variadic)
    else {
        return Err(ComptimeError::NotComptime(
            "a runtime-pack spread requires a variadic target".to_string(),
        ));
    };
    let parameter = &params[pack_index];
    if !matches!(&parameter.ty, Type::Named(name, arguments)
        if name.starts_with('*') && arguments.is_empty())
    {
        return Err(ComptimeError::NotComptime(
            "a heterogeneous runtime-pack spread requires a type-pack variadic target".to_string(),
        ));
    }
    let positional_prefix = params[..pack_index]
        .iter()
        .filter(|parameter| {
            parameter.kind == ParamKind::Regular
                && !matches!(parameter.convention, Some(crate::ast::ArgConvention::Out))
        })
        .count();
    let spread_index = spread_indices[0];
    if spread_index != positional_prefix || arguments.len() != positional_prefix + 1 {
        return Err(ComptimeError::NotComptime(
            "a runtime-pack spread must follow the fully supplied fixed positional prefix and cannot be mixed with explicit overflow arguments"
                .to_string(),
        ));
    }
    Ok(true)
}

fn unwrap_forwarded_pack_arguments(arguments: Vec<Expr>) -> Vec<Expr> {
    arguments
        .into_iter()
        .map(|argument| match argument.kind {
            ExprKind::Spread(value) => *value,
            _ => argument,
        })
        .collect()
}

fn select_whole_pack_abi(specialization: &mut Stmt) -> Result<(), ComptimeError> {
    let StmtKind::Def { params, .. } = &mut specialization.kind else {
        unreachable!("nested specializations are functions")
    };
    let Some(parameter) = params
        .iter_mut()
        .find(|parameter| parameter.kind == ParamKind::Variadic)
    else {
        return Err(ComptimeError::NotComptime(
            "whole-pack forwarding requires a variadic target".to_string(),
        ));
    };
    let Type::Named(name, _) = &mut parameter.ty else {
        return Err(ComptimeError::NotComptime(
            "whole-pack forwarding lost its concrete collector type".to_string(),
        ));
    };
    if parameter.kind != ParamKind::Variadic || name != "$pack" {
        return Err(ComptimeError::NotComptime(
            "whole-pack forwarding requires a specialized runtime pack".to_string(),
        ));
    }
    parameter.kind = ParamKind::Regular;
    *name = "__RuntimeTuple".to_string();
    Ok(())
}

fn forwarded_pack_types(
    template: &Stmt,
    display_name: &str,
    arguments: &[Expr],
    kwargs: &[crate::ast::KwArg],
    runtime_packs: &RuntimePackEnv,
) -> Result<Option<Vec<Ty>>, ComptimeError> {
    if !arguments
        .iter()
        .any(|argument| spread_source(argument).is_some())
    {
        return Ok(None);
    }
    let mut types = Vec::new();
    for argument in arguments {
        if let Some((name, _)) = spread_source(argument) {
            let pack = runtime_packs.resolve_pack(name).ok_or_else(|| {
                ComptimeError::NotComptime(format!(
                    "cannot forward '{name}' because it is not a specialized runtime pack"
                ))
            })?;
            for ty in pack {
                types.push(forwarded_source_type(ty).ok_or_else(|| {
                    ComptimeError::NotComptime(format!(
                        "cannot recover the checked type of forwarded pack element '{ty:?}'"
                    ))
                })?);
            }
        } else {
            types.push(infer_pack_argument_type(argument)?);
        }
    }
    let pack_indices =
        runtime_pack_call_argument_indices(template, display_name, types.len(), kwargs)?;
    Ok(Some(
        pack_indices
            .into_iter()
            .map(|index| types[index].clone())
            .collect(),
    ))
}

fn forwarded_source_type(source: &Type) -> Option<Ty> {
    ct_param_source_type(source).or_else(|| match source {
        Type::Named(name, arguments) => arguments
            .iter()
            .map(forwarded_source_type_argument)
            .collect::<Option<Vec<_>>>()
            .map(|arguments| Ty::Struct(name.clone(), arguments)),
        _ => None,
    })
}

fn forwarded_source_type_argument(argument: &ParamArg) -> Option<TyArg> {
    match argument {
        ParamArg::Type(ty) => forwarded_source_type(ty).map(TyArg::Ty),
        ParamArg::Value(value) => literal_ct_value(value).map(TyArg::Val),
        ParamArg::Named { value, .. } => forwarded_source_type_argument(value),
    }
}

impl Elab<'_> {
    pub(super) fn monomorphize_nested_program(
        &self,
        program: &mut [Stmt],
    ) -> Result<(), ComptimeError> {
        for statement in program {
            match &mut statement.kind {
                StmtKind::Def {
                    name, params, body, ..
                } => {
                    let parent = format!("{name}${}${}", statement.span.0, statement.span.1);
                    self.monomorphize_nested_body(parent, params, false, body)?;
                }
                StmtKind::Struct { name, methods, .. } => {
                    for (index, method) in methods.iter_mut().enumerate() {
                        let parent = format!(
                            "{name}.{}${}${}${index}",
                            method.name, statement.span.0, statement.span.1
                        );
                        self.monomorphize_nested_body(
                            parent,
                            &method.params,
                            method.has_self,
                            &mut method.body,
                        )?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn monomorphize_nested_body(
        &self,
        parent: String,
        parameters: &[crate::ast::FnParam],
        has_self: bool,
        body: &mut Vec<Stmt>,
    ) -> Result<(), ComptimeError> {
        let mut nested = NestedMono::new(parent, parameters, has_self);
        nested.qualify_root(body);
        if nested.templates.is_empty() {
            return Ok(());
        }
        let mut packs = RuntimePackEnv::new(runtime_pack_types(parameters));
        nested.scan_root(self, body, &mut packs)?;
        nested.drain(self)?;
        nested.replace_templates(body);
        debug_assert!(nested.generated.is_empty());
        Ok(())
    }
}

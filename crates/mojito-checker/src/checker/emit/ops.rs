//! The op an emitted node holds from its allocation: its syntax's
//! operation, each operand the node the emitter allocated for it.
//!
//! It is what the checked arena's builder (`mojito_checked::checked`'s
//! `ArenaBuilder`) makes of the same syntax, arm for arm, with every field
//! the check decides later left empty for the facts on the node to fill:
//! a declaration's symbol is its written name and its signature the
//! default, a call holds one parameter per written bracket argument, a
//! subscript's key is its index's node, a type value holds no type, a
//! `case`'s and a `match`'s plan is absent, and a destructuring loop's
//! binder is unnamed until its desugar is spliced. The arena builder stays
//! the oracle `MOJITO_CHECK_EMITTED=1` compares these ops with until the
//! emitted nodes replace it, so the two mappings live side by side.

use super::EmittedSlot;
use mojito_ast::ast::{
    BindingMode, ComprehensionClause, Condition, DestructureTarget, DestructureTargetKind, Expr,
    ExprKind, FnParam, KwArg, MatchCase, MatchPattern, MatchPatternKind, ParamArg, SequencePattern,
    SequenceTarget, Stmt, StmtKind, SubscriptArg, TStringPart,
};
use mojito_ast::visit::{self, Visitor};
use mojito_checked::checked::{
    CheckedComprehensionClause, CheckedCondition, CheckedDef, CheckedField, CheckedFnParam,
    CheckedKwArg, CheckedMatchCase, CheckedMethod, CheckedNodeId, CheckedOp, CheckedParamArg,
    CheckedParameter, CheckedPattern, CheckedPatternKind, CheckedSequencePattern,
    CheckedSequenceTarget, CheckedSignature, CheckedStmtId, CheckedStmtOp, CheckedStruct,
    CheckedStructComptime, CheckedSubscriptArg, CheckedTStringPart, CheckedTarget,
    CheckedTargetKind, CheckedTrait, CheckedTraitComptime, CheckedTraitDefault, CheckedTraitMethod,
    CheckedWithItem, MatchEmission,
};
use mojito_checked::declaration::InlineLevel;
use mojito_common::token::SourceSpan;
use std::collections::HashMap;

/// An expression op and its candidate lists
/// ([`mojito_checked::emitted::EmittedExpr::applications`]).
pub(super) type ExpressionOp = (CheckedOp, Vec<Vec<CheckedNodeId>>);

/// A statement op and its candidate list.
pub(super) type StatementOp = (CheckedStmtOp, Vec<CheckedNodeId>);

/// An operand of the syntax being given its op has no node.
#[derive(Debug, Clone, Copy)]
pub(super) struct Missing;

/// The ops of syntax whose every operand `index` holds a node for.
pub(super) struct Ops<'a> {
    pub(super) index: &'a HashMap<SourceSpan, EmittedSlot>,
}

impl Ops<'_> {
    /// `expression`'s op, or `Missing` when an operand has no node.
    pub(super) fn expression(&self, expression: &Expr) -> Result<ExpressionOp, Missing> {
        let mut applications = Vec::new();
        let op = match &expression.kind {
            ExprKind::Int(value) => CheckedOp::Int(value.clone()),
            ExprKind::Float(value) => CheckedOp::Float(value.clone()),
            ExprKind::Bool(value) => CheckedOp::Bool(*value),
            ExprKind::Str(value) => CheckedOp::Str(value.clone()),
            ExprKind::None => CheckedOp::None,
            ExprKind::Uninitialized => CheckedOp::Uninitialized,
            ExprKind::EmptySubscript => CheckedOp::EmptySubscript,
            ExprKind::IsRunInComptimeInterpreter => CheckedOp::IsRunInComptimeInterpreter,
            ExprKind::Identifier(name) => CheckedOp::Identifier(name.clone()),
            ExprKind::FunctionType(_) | ExprKind::GeneratorType { .. } => {
                applications.push(self.candidates(|found| visit::walk_expr(found, expression)));
                CheckedOp::TypeValue {
                    ty: None,
                    applications: Vec::new(),
                }
            }
            ExprKind::Prefix(op, value) => CheckedOp::Prefix(*op, self.expr(value)?),
            ExprKind::Transfer(value) => CheckedOp::Transfer(self.expr(value)?),
            ExprKind::Spread(value) => CheckedOp::Spread(self.expr(value)?),
            ExprKind::Named { target, value } => CheckedOp::Named {
                target: self.expr(target)?,
                value: self.expr(value)?,
            },
            ExprKind::Infix(op, left, right) => {
                CheckedOp::Infix(*op, self.expr(left)?, self.expr(right)?)
            }
            ExprKind::Index { object, index } => {
                applications.push(self.candidates(|found| visit::walk_expr(found, index)));
                CheckedOp::Index {
                    object: self.expr(object)?,
                    index: CheckedParamArg::Value(self.expr(index)?),
                }
            }
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } => CheckedOp::Call {
                name: name.clone(),
                param_args: self.param_args(param_args, &mut applications)?,
                args: self.values(args)?,
                kwargs: self.kwargs(kwargs)?,
            },
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } => CheckedOp::Invoke {
                callee: self.expr(callee)?,
                param_args: self.param_args(param_args, &mut applications)?,
                args: self.values(args)?,
                kwargs: self.kwargs(kwargs)?,
            },
            ExprKind::Member { object, field } => CheckedOp::Member {
                object: self.expr(object)?,
                field: field.clone(),
            },
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } => CheckedOp::MethodCall {
                object: self.expr(object)?,
                method: method.clone(),
                args: self.values(args)?,
                kwargs: self.kwargs(kwargs)?,
            },
            ExprKind::ListLit(items) => CheckedOp::ListLit(self.values(items)?),
            ExprKind::TupleLit(items) => CheckedOp::TupleLit(self.values(items)?),
            ExprKind::BraceLit(entries) => CheckedOp::BraceLit(
                entries
                    .iter()
                    .map(|(key, value)| Ok((self.expr(key)?, self.optional(value.as_ref())?)))
                    .collect::<Result<_, _>>()?,
            ),
            ExprKind::InitializerList { args, kwargs } => CheckedOp::InitializerList {
                args: self.values(args)?,
                kwargs: self.kwargs(kwargs)?,
            },
            ExprKind::Comprehension {
                kind,
                key,
                value,
                clauses,
            } => CheckedOp::Comprehension {
                kind: *kind,
                key: self.optional(key.as_deref())?,
                value: self.expr(value)?,
                clauses: clauses
                    .iter()
                    .map(|clause| self.comprehension_clause(clause))
                    .collect::<Result<_, _>>()?,
            },
            ExprKind::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => CheckedOp::IfExpr {
                cond: self.expr(cond)?,
                then_branch: self.expr(then_branch)?,
                else_branch: self.expr(else_branch)?,
            },
            ExprKind::Compare { first, rest } => CheckedOp::Compare {
                first: self.expr(first)?,
                rest: rest
                    .iter()
                    .map(|(op, value)| Ok((*op, self.expr(value)?)))
                    .collect::<Result<_, _>>()?,
            },
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
            } => CheckedOp::Slice {
                object: self.expr(object)?,
                lower: self.optional(lower.as_deref())?,
                upper: self.optional(upper.as_deref())?,
                step: self.optional(step.as_deref())?,
            },
            ExprKind::MultiIndex { object, args } => CheckedOp::MultiIndex {
                object: self.expr(object)?,
                args: args
                    .iter()
                    .map(|argument| self.subscript_arg(argument))
                    .collect::<Result<_, _>>()?,
            },
            ExprKind::TString { parts, raw } => CheckedOp::TString {
                parts: parts
                    .iter()
                    .map(|part| {
                        Ok(match part {
                            TStringPart::Literal(text) => CheckedTStringPart::Literal(text.clone()),
                            TStringPart::Expr(value) => CheckedTStringPart::Expr(self.expr(value)?),
                        })
                    })
                    .collect::<Result<_, _>>()?,
                raw: *raw,
            },
            ExprKind::Lambda { def } => CheckedOp::Lambda {
                def: self.stmt(def)?,
            },
        };
        Ok((op, applications))
    }

    /// `statement`'s op, or `Missing` when an operand has no node.
    pub(super) fn statement(&self, statement: &Stmt) -> Result<StatementOp, Missing> {
        let mut applications = Vec::new();
        let op = match &statement.kind {
            StmtKind::Def { .. } | StmtKind::Struct { .. } | StmtKind::Trait { .. } => {
                self.declaration(statement)?
            }
            StmtKind::VarDecl { name, ty, value } => {
                if let Some(ty) = ty {
                    applications = self.candidates(|found| visit::walk_annotation(found, ty));
                }
                CheckedStmtOp::VarDecl {
                    name: name.clone(),
                    applications: Vec::new(),
                    value: self.expr(value)?,
                }
            }
            StmtKind::RefDecl { name, value } => CheckedStmtOp::RefDecl {
                name: name.clone(),
                value: self.expr(value)?,
            },
            StmtKind::Assign { name, value } => CheckedStmtOp::Assign {
                name: name.clone(),
                value: self.expr(value)?,
            },
            StmtKind::Raise(value) => CheckedStmtOp::Raise(self.expr(value)?),
            StmtKind::Expr(value) if let Some((cond, message)) = value.kgen_param_assert() => {
                CheckedStmtOp::ComptimeAssert {
                    cond: self.expr(cond)?,
                    message: Some(self.expr(message)?),
                    denotes: None,
                    fails: false,
                }
            }
            StmtKind::Expr(value) => CheckedStmtOp::Expr(self.expr(value)?),
            StmtKind::Comptime {
                name,
                where_clauses,
                value,
                ..
            } => CheckedStmtOp::Comptime {
                name: name.clone(),
                where_clauses: self.values(where_clauses)?,
                value: self.expr(value)?,
                denotes: None,
            },
            StmtKind::AugAssign { place, op, value } => CheckedStmtOp::AugAssign {
                place: self.expr(place)?,
                op: *op,
                value: self.expr(value)?,
            },
            StmtKind::SetPlace { place, value } => CheckedStmtOp::SetPlace {
                place: self.expr(place)?,
                value: self.expr(value)?,
            },
            StmtKind::Destructure { target, value } => CheckedStmtOp::Destructure {
                target: self.target(target)?,
                value: self.expr(value)?,
            },
            StmtKind::Assert { .. } => CheckedStmtOp::Pass,
            StmtKind::ComptimeIf { branches, orelse } => CheckedStmtOp::ComptimeIf {
                branches: self.branches(branches)?,
                orelse: self.optional_block(orelse.as_deref())?,
            },
            StmtKind::If { branches, orelse } => CheckedStmtOp::If {
                branches: self.branches(branches)?,
                orelse: self.optional_block(orelse.as_deref())?,
            },
            StmtKind::ComptimeFor { var, iter, body } => CheckedStmtOp::ComptimeFor {
                var: var.clone(),
                iter: self.expr(iter)?,
                body: self.block(body)?,
            },
            StmtKind::ComptimeAssert { cond, message } => CheckedStmtOp::ComptimeAssert {
                cond: self.expr(cond)?,
                message: self.optional(message.as_ref())?,
                denotes: None,
                fails: false,
            },
            StmtKind::Scope(body) => CheckedStmtOp::Scope(self.block(body)?),
            StmtKind::While { cond, body, orelse } => CheckedStmtOp::While {
                cond: self.condition(cond)?,
                body: self.block(body)?,
                orelse: self.optional_block(orelse.as_deref())?,
            },
            StmtKind::Match { cases, .. } => CheckedStmtOp::Match {
                anchor: None,
                cases: self.cases(cases)?,
                emission: MatchEmission::default(),
            },
            StmtKind::ComptimeMatch { cases, .. } => CheckedStmtOp::ComptimeMatch {
                anchor: None,
                cases: self.cases(cases)?,
            },
            StmtKind::For {
                target,
                iter,
                body,
                orelse,
            } => {
                let (var, binding) = loop_binding(target);
                CheckedStmtOp::For {
                    var,
                    binding,
                    iter: self.expr(iter)?,
                    body: self.block(body)?,
                    orelse: self.optional_block(orelse.as_deref())?,
                }
            }
            StmtKind::Return(value) => CheckedStmtOp::Return(self.optional(value.as_ref())?),
            StmtKind::With { items, body } => CheckedStmtOp::With {
                items: items
                    .iter()
                    .map(|item| {
                        Ok(CheckedWithItem {
                            context: self.expr(&item.context)?,
                            var: item.var.clone(),
                        })
                    })
                    .collect::<Result<_, _>>()?,
                body: self.block(body)?,
            },
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => CheckedStmtOp::Try {
                body: self.block(body)?,
                except: except
                    .as_ref()
                    .map(|(name, body)| Ok((name.clone(), self.block(body)?)))
                    .transpose()?,
                orelse: self.optional_block(orelse.as_deref())?,
                finalbody: self.optional_block(finalbody.as_deref())?,
            },
            StmtKind::Import { path, alias } => CheckedStmtOp::Import {
                path: path.clone(),
                alias: alias.clone(),
            },
            StmtKind::FromImport { level, path, names } => CheckedStmtOp::FromImport {
                level: *level,
                path: path.clone(),
                names: names.clone(),
            },
            StmtKind::Pass => CheckedStmtOp::Pass,
            StmtKind::Break => CheckedStmtOp::Break,
            StmtKind::Continue => CheckedStmtOp::Continue,
        };
        Ok((op, applications))
    }

    fn expr(&self, expression: &Expr) -> Result<CheckedNodeId, Missing> {
        match self.index.get(&expression.source_span()) {
            Some(EmittedSlot::Expression(id)) => Ok(*id),
            _ => Err(Missing),
        }
    }

    fn values(&self, values: &[Expr]) -> Result<Vec<CheckedNodeId>, Missing> {
        values.iter().map(|value| self.expr(value)).collect()
    }

    fn optional(&self, expression: Option<&Expr>) -> Result<Option<CheckedNodeId>, Missing> {
        expression
            .map(|expression| self.expr(expression))
            .transpose()
    }

    fn block(&self, statements: &[Stmt]) -> Result<Vec<CheckedStmtId>, Missing> {
        statements
            .iter()
            .map(|statement| self.stmt(statement))
            .collect()
    }

    fn pattern(&self, pattern: &MatchPattern) -> Result<CheckedPattern, Missing> {
        let patterns = |items: &[MatchPattern]| -> Result<Vec<CheckedPattern>, Missing> {
            items.iter().map(|item| self.pattern(item)).collect()
        };
        let named =
            |items: &[(String, MatchPattern)]| -> Result<Vec<(String, CheckedPattern)>, Missing> {
                items
                    .iter()
                    .map(|(name, pattern)| Ok((name.clone(), self.pattern(pattern)?)))
                    .collect()
            };
        let kind = match &pattern.kind {
            MatchPatternKind::Wildcard => CheckedPatternKind::Wildcard,
            MatchPatternKind::Bind { name, mode } => CheckedPatternKind::Bind {
                name: name.clone(),
                mode: *mode,
            },
            MatchPatternKind::Literal { value, .. } => {
                CheckedPatternKind::Literal(self.expr(value)?)
            }
            MatchPatternKind::Equal { value, .. } => CheckedPatternKind::Equal(self.expr(value)?),
            MatchPatternKind::Head(value) => CheckedPatternKind::Head(self.expr(value)?),
            MatchPatternKind::Tuple(items) => CheckedPatternKind::Tuple(patterns(items)?),
            MatchPatternKind::Or(items) => CheckedPatternKind::Or(patterns(items)?),
            MatchPatternKind::Sequence(items) => CheckedPatternKind::Sequence(
                items
                    .iter()
                    .map(|item| {
                        Ok(match item {
                            SequencePattern::Element(pattern) => {
                                CheckedSequencePattern::Element(self.pattern(pattern)?)
                            }
                            SequencePattern::Star(pattern) => {
                                CheckedSequencePattern::Star(self.pattern(pattern)?)
                            }
                        })
                    })
                    .collect::<Result<_, _>>()?,
            ),
            MatchPatternKind::Call { head, args, kwargs } => CheckedPatternKind::Call {
                head: self.expr(head)?,
                args: patterns(args)?,
                kwargs: named(kwargs)?,
            },
            MatchPatternKind::Fields(fields) => CheckedPatternKind::Fields(named(fields)?),
            MatchPatternKind::As { pattern, name } => CheckedPatternKind::As {
                pattern: Box::new(self.pattern(pattern)?),
                name: name.clone(),
            },
        };
        Ok(CheckedPattern {
            kind,
            span: pattern.span,
            syntax_id: pattern.syntax_id,
        })
    }

    fn stmt(&self, statement: &Stmt) -> Result<CheckedStmtId, Missing> {
        match self.index.get(&statement.source_span()) {
            Some(EmittedSlot::Statement(id)) => Ok(*id),
            _ => Err(Missing),
        }
    }

    fn kwargs(&self, kwargs: &[KwArg]) -> Result<Vec<CheckedKwArg>, Missing> {
        kwargs
            .iter()
            .map(|kwarg| {
                Ok(CheckedKwArg {
                    name: kwarg.name.clone(),
                    value: self.expr(&kwarg.value)?,
                })
            })
            .collect()
    }

    fn fn_params(&self, params: &[FnParam]) -> Result<Vec<CheckedFnParam>, Missing> {
        params
            .iter()
            .map(|param| {
                Ok(CheckedFnParam {
                    name: param.name.clone(),
                    default: self.optional(param.default.as_ref())?,
                    kind: param.kind,
                    convention: param.convention,
                })
            })
            .collect()
    }

    fn optional_block(&self, body: Option<&[Stmt]>) -> Result<Option<Vec<CheckedStmtId>>, Missing> {
        body.map(|body| self.block(body)).transpose()
    }

    fn condition(&self, condition: &Condition) -> Result<CheckedCondition, Missing> {
        Ok(match condition {
            Condition::Expr(value) => CheckedCondition::Expr(self.expr(value)?),
            Condition::Case { pattern, value } => CheckedCondition::Case {
                value: self.expr(value)?,
                pattern: self.pattern(pattern)?,
                anchor: None,
                commands: Vec::new(),
                reads: Vec::new(),
            },
        })
    }

    fn branches(
        &self,
        branches: &[(Condition, Vec<Stmt>)],
    ) -> Result<Vec<(CheckedCondition, Vec<CheckedStmtId>)>, Missing> {
        branches
            .iter()
            .map(|(condition, body)| Ok((self.condition(condition)?, self.block(body)?)))
            .collect()
    }

    /// A match's cases as written: each guard its node, every planned part
    /// absent.
    fn cases(&self, cases: &[MatchCase]) -> Result<Vec<CheckedMatchCase>, Missing> {
        cases
            .iter()
            .map(|case| {
                Ok(CheckedMatchCase {
                    pattern: self.pattern(&case.pattern)?,
                    commands: Vec::new(),
                    reads: Vec::new(),
                    guard: self.optional(case.guard.as_ref())?,
                    condition: None,
                    body: self.block(&case.body)?,
                    unreachable: false,
                })
            })
            .collect()
    }

    fn target(&self, target: &DestructureTarget) -> Result<CheckedTarget, Missing> {
        let kind = match &target.kind {
            DestructureTargetKind::Discard => CheckedTargetKind::Discard,
            DestructureTargetKind::Declare { name, mode } => CheckedTargetKind::Declare {
                name: name.clone(),
                mode: *mode,
                binding: None,
                binding_ty: None,
            },
            DestructureTargetKind::Place(place) => CheckedTargetKind::Place(self.expr(place)?),
            DestructureTargetKind::Tuple(items) => CheckedTargetKind::Tuple(
                items
                    .iter()
                    .map(|item| self.target(item))
                    .collect::<Result<_, _>>()?,
            ),
            DestructureTargetKind::Sequence(items) => CheckedTargetKind::Sequence(
                items
                    .iter()
                    .map(|item| {
                        Ok(match item {
                            SequenceTarget::Element(target) => {
                                CheckedSequenceTarget::Element(self.target(target)?)
                            }
                            SequenceTarget::Star(target) => {
                                CheckedSequenceTarget::Star(self.target(target)?)
                            }
                        })
                    })
                    .collect::<Result<_, _>>()?,
            ),
        };
        Ok(CheckedTarget {
            kind,
            span: target.span,
            syntax_id: target.syntax_id,
        })
    }

    /// A declaration's op, its symbol its written name (a method's
    /// `Owner.method`) and its signature the default.
    fn declaration(&self, statement: &Stmt) -> Result<CheckedStmtOp, Missing> {
        Ok(match &statement.kind {
            StmtKind::Def {
                name,
                params,
                positional_only,
                keyword_only,
                raises,
                where_clauses,
                body,
                decorators,
                captures,
                ..
            } => CheckedStmtOp::Def(Box::new(CheckedDef {
                name: name.clone(),
                symbol: name.clone(),
                signature: CheckedSignature::default(),
                params: self.fn_params(params)?,
                positional_only: *positional_only,
                keyword_only: *keyword_only,
                raises: *raises,
                where_clauses: self.values(where_clauses)?,
                body: self.block(body)?,
                captures: Vec::new(),
                capture_list: captures.is_some(),
                storage: None,
                inline: InlineLevel::of(decorators).unwrap_or_default(),
                unavailable: mojito_ast::ast::is_unavailable(decorators),
            })),
            StmtKind::Struct {
                name,
                conforms,
                conformance_conditions,
                where_clauses,
                fields,
                associated,
                methods,
                fieldwise_init,
                ..
            } => CheckedStmtOp::Struct(Box::new(CheckedStruct {
                name: name.clone(),
                conforms: entry_names(conforms),
                conformance_conditions: conformance_conditions
                    .iter()
                    .filter_map(|(entry, value)| {
                        let entry = entry.identifier()?.to_string();
                        Some(self.expr(value).map(|value| (entry, value)))
                    })
                    .collect::<Result<_, _>>()?,
                where_clauses: self.values(where_clauses)?,
                fields: fields
                    .iter()
                    .map(|declared| CheckedField {
                        name: declared.name.clone(),
                        ty: None,
                        fieldwise_convention: None,
                        fieldwise_ref: None,
                        reference: declared.reference.is_some(),
                    })
                    .collect(),
                associated: associated
                    .iter()
                    .map(|member| {
                        Ok(CheckedStructComptime {
                            name: member.name.clone(),
                            where_clauses: self.values(&member.where_clauses)?,
                            value: self.expr(&member.value)?,
                        })
                    })
                    .collect::<Result<_, _>>()?,
                methods: methods
                    .iter()
                    .map(|method| {
                        let registered =
                            mojito_symbol::symbol::lifecycle_method_name(method).to_string();
                        Ok(CheckedMethod {
                            name: method.name.clone(),
                            symbol: format!("{name}.{registered}"),
                            registered,
                            signature: CheckedSignature::default(),
                            has_self: method.has_self,
                            self_convention: method.self_convention,
                            params: self.fn_params(&method.params)?,
                            positional_only: method.positional_only,
                            keyword_only: method.keyword_only,
                            raises: method.raises,
                            where_clauses: self.values(&method.where_clauses)?,
                            body: self.block(&method.body)?,
                            provenance: method.provenance.clone(),
                            inline: InlineLevel::of(&method.decorators).unwrap_or_default(),
                        })
                    })
                    .collect::<Result<_, _>>()?,
                fieldwise_init: *fieldwise_init,
            })),
            StmtKind::Trait {
                name,
                refines,
                methods,
                comptime_members,
            } => CheckedStmtOp::Trait(Box::new(CheckedTrait {
                name: name.clone(),
                refines: entry_names(refines),
                methods: methods
                    .iter()
                    .map(|method| {
                        Ok(CheckedTraitMethod {
                            name: method.name.clone(),
                            symbol: format!("{name}.{}", method.name),
                            signature: CheckedSignature::default(),
                            self_convention: method.self_convention,
                            params: self.fn_params(&method.params)?,
                            positional_only: method.positional_only,
                            keyword_only: method.keyword_only,
                            raises: method.raises,
                            where_clauses: self.values(&method.where_clauses)?,
                            default: method
                                .default_body
                                .as_ref()
                                .map(|body| {
                                    Ok(CheckedTraitDefault {
                                        body: self.block(body)?,
                                        inline: InlineLevel::of(&method.decorators)
                                            .unwrap_or_default(),
                                    })
                                })
                                .transpose()?,
                        })
                    })
                    .collect::<Result<_, _>>()?,
                comptime_members: comptime_members
                    .iter()
                    .map(|member| {
                        Ok(CheckedTraitComptime {
                            name: member.name.clone(),
                            where_clauses: self.values(&member.where_clauses)?,
                        })
                    })
                    .collect::<Result<_, _>>()?,
            })),
            _ => return Err(Missing),
        })
    }

    /// One parameter per written bracket argument, in source order, each
    /// bound to no declaration; each argument's candidates are pushed onto
    /// `applications`.
    fn param_args(
        &self,
        arguments: &[ParamArg],
        applications: &mut Vec<Vec<CheckedNodeId>>,
    ) -> Result<Vec<CheckedParameter>, Missing> {
        arguments
            .iter()
            .map(|argument| {
                let value = innermost(argument);
                applications.push(if matches!(value.kind, ExprKind::Lambda { .. }) {
                    self.expr(value).into_iter().collect()
                } else {
                    self.candidates(|found| visit::walk_expr(found, value))
                });
                Ok(CheckedParameter {
                    declaration: None,
                    arguments: vec![self.param_arg(argument)?],
                })
            })
            .collect()
    }

    fn param_arg(&self, argument: &ParamArg) -> Result<CheckedParamArg, Missing> {
        Ok(match argument {
            ParamArg::Value(value) => CheckedParamArg::Value(self.expr(value)?),
            ParamArg::Named { name, value } => CheckedParamArg::Named {
                name: name.clone(),
                value: Box::new(self.param_arg(value)?),
            },
        })
    }

    fn comprehension_clause(
        &self,
        clause: &ComprehensionClause,
    ) -> Result<CheckedComprehensionClause, Missing> {
        Ok(match clause {
            ComprehensionClause::For { target, iter, .. } => {
                let (var, binding) = loop_binding(target);
                CheckedComprehensionClause::For {
                    var,
                    binding,
                    iter: self.expr(iter)?,
                    unpack: None,
                }
            }
            ComprehensionClause::If(condition) => {
                CheckedComprehensionClause::If(self.expr(condition)?)
            }
        })
    }

    fn subscript_arg(&self, argument: &SubscriptArg) -> Result<CheckedSubscriptArg, Missing> {
        Ok(match argument {
            SubscriptArg::Index(value) => CheckedSubscriptArg::Index(self.expr(value)?),
            SubscriptArg::Keyword { name, value } => CheckedSubscriptArg::Keyword {
                name: name.clone(),
                value: self.expr(value)?,
            },
            SubscriptArg::Slice { lower, upper, step } => CheckedSubscriptArg::Slice {
                lower: self.optional(lower.as_deref())?,
                upper: self.optional(upper.as_deref())?,
                step: self.optional(step.as_deref())?,
            },
            SubscriptArg::KeywordSlice {
                name,
                lower,
                upper,
                step,
            } => CheckedSubscriptArg::KeywordSlice {
                name: name.clone(),
                lower: self.optional(lower.as_deref())?,
                upper: self.optional(upper.as_deref())?,
                step: self.optional(step.as_deref())?,
            },
        })
    }

    /// The nodes of the expressions `walk` reports, in its order.
    fn candidates(&self, walk: impl FnOnce(&mut Candidates<'_, '_>)) -> Vec<CheckedNodeId> {
        let mut found = Candidates {
            ops: self,
            found: Vec::new(),
        };
        walk(&mut found);
        found.found
    }
}

/// The nodes of the expressions a walk reports.
struct Candidates<'o, 'a> {
    ops: &'o Ops<'a>,
    found: Vec<CheckedNodeId>,
}

impl Visitor for Candidates<'_, '_> {
    fn visit_expr(&mut self, expression: &Expr) {
        self.found.extend(self.ops.expr(expression).ok());
    }
}

/// A bracket argument's value under its keyword.
fn innermost(argument: &ParamArg) -> &Expr {
    match argument {
        ParamArg::Value(value) => value,
        ParamArg::Named { value, .. } => innermost(value),
    }
}

/// A loop target's binder: its one declared leaf, or an unnamed one for a
/// destructuring target, which binds an immutable reference to each piece
/// once its desugar is spliced.
fn loop_binding(target: &DestructureTarget) -> (String, BindingMode) {
    target.loop_binding().map_or_else(
        || (String::new(), BindingMode::Immutable),
        |(name, mode)| (name.to_string(), mode),
    )
}

/// The trait names of a conformance or refinement list.
fn entry_names(entries: &[Expr]) -> Vec<String> {
    mojito_ast::ast::trait_entry_names(entries)
        .map(str::to_string)
        .collect()
}

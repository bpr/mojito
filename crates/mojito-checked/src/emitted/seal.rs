//! The seal: the checked tables composed from the check's emitted nodes.
//!
//! Each emitted node holds the op its syntax gave it at allocation and the
//! facts the check set on it afterwards. The seal composes the two into the
//! op upstream's emitter would have left final: a call's parameters
//! regrouped as its binder laid them out, a subscript's key and a type
//! value's type from their facts, a declaration's symbol and signature from
//! its sites, a `match`'s and a `case`'s plan, and the check's desugars
//! spliced into every body. A spelled sugar's position holds its
//! construction. Nodes are numbered post-order, each operand before the op
//! that names it, in the order [`CheckedOp::map_operands`] visits them, and
//! a node reached twice is sealed twice: HIR, MIR, and the package read the
//! ids in that order.

use super::{
    EmittedDesugar, EmittedExpr, EmittedNodes, EmittedSlot, EmittedStmt, Unsealed, substituted,
};
use crate::checked::{
    CheckedCommand, CheckedComprehensionClause, CheckedComprehensionUnpack, CheckedCondition,
    CheckedDef, CheckedDefaultWrapper, CheckedMatchCase, CheckedMethod, CheckedNodeId, CheckedOp,
    CheckedParamArg, CheckedParameter, CheckedPathRead, CheckedSequenceTarget, CheckedStmtId,
    CheckedStmtOp, CheckedStruct, CheckedStructComptime, CheckedTables, CheckedTarget,
    CheckedTargetKind, CheckedTrait, CheckedTraitComptime, CheckedTraitDefault, CheckedTraitMethod,
    ConformanceFacts, DeclarationSites, MatchPlan, NodeFacts, NodeMap, Outermost, PlannedCommand,
    Signatures, apply_spelled, expression_node, lifted, map_params, parameter_layout,
    spelled_construction, statement_node, subscript_attribute,
};
use mojito_ast::ast::{BindingMode, Expr, Stmt};
use mojito_common::token::{SourceSpan, Span, SyntaxId};
use mojito_types::types::TyArg;
use std::cell::RefCell;

/// The checked tables of `roots` and of `wrappers`' bodies, sealed from
/// `emitted`.
///
/// Each node's op is composed with its facts, a declaration's symbol and
/// signature read from `sites` and a fieldwise constructor's from
/// `conformances`. Each wrapper's signature is set and its body, whose
/// nodes the tables hold, taken. An occurrence the check emitted no node or
/// op for leaves the tables [`Unsealed`].
pub fn seal(
    emitted: &EmittedNodes,
    roots: &[Stmt],
    wrappers: &mut [CheckedDefaultWrapper],
    sites: &DeclarationSites,
    conformances: &ConformanceFacts,
) -> Result<CheckedTables, Unsealed> {
    let expressions_span = mojito_common::timing::span("expressions");
    let signatures = Signatures {
        checked_types: &sites.checked_types,
        generic_parameters: &sites.generic_parameters,
        declaration_effects: &sites.declaration_effects,
        fieldwise_conventions: &conformances.fieldwise_conventions,
        fieldwise_ref_params: &conformances.fieldwise_ref_params,
        declaration_symbols: &sites.declaration_symbols,
    };
    for wrapper in wrappers.iter_mut() {
        wrapper.signature = signatures.method(
            wrapper.module.as_ref(),
            &wrapper.declaration,
            wrapper.method_index,
            wrapper.params.len(),
        );
    }
    let mut seal = Seal {
        emitted,
        signatures,
        nodes: Vec::new(),
        statements: Vec::new(),
        functions: 0,
        unsealed: RefCell::new(Unsealed::default()),
    };
    let roots = seal.roots(roots);
    let wrapper_roots = wrappers
        .iter_mut()
        .map(|wrapper| seal.roots(&std::mem::take(&mut wrapper.body)))
        .collect();
    let unsealed = seal.unsealed.into_inner();
    if unsealed.count > 0 {
        return Err(unsealed);
    }
    drop(expressions_span);
    Ok(CheckedTables::new(
        seal.nodes,
        seal.statements,
        roots,
        wrapper_roots,
    ))
}

/// The tables being sealed. A [`NodeMap`] from emitted slots to the sealed
/// ids it assigns.
struct Seal<'a> {
    emitted: &'a EmittedNodes,
    signatures: Signatures<'a>,
    nodes: Vec<crate::checked::CheckedExpr>,
    statements: Vec<crate::checked::CheckedStmt>,
    /// How many function bodies enclose the statement being sealed: a
    /// `def` inside one is named by its own name.
    functions: usize,
    /// The occurrences no emitted node or op holds.
    unsealed: RefCell<Unsealed>,
}

impl NodeMap for Seal<'_> {
    fn expression(&mut self, slot: CheckedNodeId) -> CheckedNodeId {
        let emitted = self.emitted;
        let Some(node) = emitted.expressions.get(slot.0 as usize) else {
            return self.missing_expression(None);
        };
        let (Some(op), Some(syntax)) = (&node.op, node.at.syntax) else {
            return self.missing_expression(Some(&node.at));
        };
        let facts = held(node.facts.as_deref());
        if substituted(op)
            && let Some((construction, lifted)) = spelled_construction(facts)
        {
            let Some(construction) = self.expression_slot(construction) else {
                return self.missing_expression(Some(&construction.source_span()));
            };
            let id = self.expression(construction);
            apply_spelled(&mut self.nodes[id.0 as usize], facts, lifted);
            return id;
        }
        let bracket_base = self.bracket_base(op);
        let op = self.shape_expression(node, op, facts).map_operands(self);
        let id = CheckedNodeId(self.nodes.len() as u32);
        self.nodes.push(expression_node(
            id,
            op,
            (node.at.span, node.at.source.clone(), syntax),
            facts,
            bracket_base,
        ));
        id
    }

    fn statement(&mut self, slot: CheckedStmtId) -> CheckedStmtId {
        let emitted = self.emitted;
        let Some(node) = emitted.statements.get(slot.0 as usize) else {
            return self.missing_statement(None);
        };
        let (Some(op), Some(syntax)) = (&node.op, node.at.syntax) else {
            return self.missing_statement(Some(&node.at));
        };
        let facts = held(node.facts.as_deref());
        let op = match op {
            CheckedStmtOp::Def(def) => {
                CheckedStmtOp::Def(Box::new(self.def(&node.at, syntax, def, facts)))
            }
            CheckedStmtOp::Struct(declaration) => {
                CheckedStmtOp::Struct(Box::new(self.structure(&node.at, declaration)))
            }
            CheckedStmtOp::Trait(declaration) => {
                CheckedStmtOp::Trait(Box::new(self.trait_declaration(&node.at, declaration)))
            }
            op => self.shape_statement(node, op, facts).map_operands(self),
        };
        let id = CheckedStmtId(self.statements.len() as u32);
        self.statements.push(statement_node(
            id,
            op,
            (node.at.span, node.at.source.clone(), syntax),
            facts,
        ));
        id
    }
}

impl<'a> Seal<'a> {
    /// The facts recorded at `at`, an occurrence the seal reaches by its
    /// written syntax rather than by a node: a destructuring target's leaf
    /// or a `case` pattern, whose record is detached (R2569).
    fn facts(&self, at: &SourceSpan) -> &'a NodeFacts {
        let emitted = self.emitted;
        held(emitted.facts(at))
    }

    /// The emitted slot of the expression `expression`.
    fn expression_slot(&self, expression: &Expr) -> Option<CheckedNodeId> {
        match self.emitted.index.get(&expression.source_span())? {
            EmittedSlot::Expression(id) => Some(*id),
            EmittedSlot::Statement(_) => None,
        }
    }

    /// The emitted slot of the planned expression `expression`, or one no
    /// node holds, which seals as missing at `expression`.
    fn planned_expression(&self, expression: &Expr) -> CheckedNodeId {
        self.expression_slot(expression).unwrap_or_else(|| {
            self.unsealed.borrow_mut().locate(&expression.source_span());
            CheckedNodeId(u32::MAX)
        })
    }

    /// The emitted slot of the planned statement `statement`, or one no
    /// node holds, which seals as missing at `statement`.
    fn planned_statement(&self, statement: &Stmt) -> CheckedStmtId {
        if let Some(EmittedSlot::Statement(id)) = self.emitted.index.get(&statement.source_span()) {
            *id
        } else {
            self.unsealed.borrow_mut().locate(&statement.source_span());
            CheckedStmtId(u32::MAX)
        }
    }

    /// The outermost lifted applications among `candidates`, the emitted
    /// nodes under an operand in walk order.
    fn outermost(&self, candidates: Option<&Vec<CheckedNodeId>>) -> Vec<CheckedNodeId> {
        let mut outermost = Outermost::default();
        candidates
            .into_iter()
            .flatten()
            .copied()
            .filter(|candidate| {
                self.emitted
                    .expressions
                    .get(candidate.0 as usize)
                    .is_some_and(|node| {
                        outermost.admits(node.at.span, lifted(held(node.facts.as_deref())))
                    })
            })
            .collect()
    }

    /// The sealed nodes of a body's statements, its desugars spliced.
    fn block(&mut self, body: &[CheckedStmtId]) -> Vec<CheckedStmtId> {
        self.spliced(body)
            .into_iter()
            .map(|statement| self.statement(statement))
            .collect()
    }

    /// `body` with each `with` statement and sequence destructuring the
    /// check desugared replaced by its desugar's statements, spliced in
    /// turn.
    fn spliced(&self, body: &[CheckedStmtId]) -> Vec<CheckedStmtId> {
        let mut spliced = Vec::new();
        for statement in body {
            self.splice(*statement, &mut spliced);
        }
        spliced
    }

    fn splice(&self, statement: CheckedStmtId, spliced: &mut Vec<CheckedStmtId>) {
        match self.statement_desugar(statement) {
            Some(desugar) => {
                for replacement in &desugar.statements {
                    self.splice(*replacement, spliced);
                }
            }
            None => spliced.push(statement),
        }
    }

    /// The desugar that replaces `statement`, a `with` statement or a
    /// destructuring.
    fn statement_desugar(&self, statement: CheckedStmtId) -> Option<&'a EmittedDesugar> {
        let emitted = self.emitted;
        let node = emitted.statements.get(statement.0 as usize)?;
        if !matches!(
            node.op,
            Some(CheckedStmtOp::With { .. } | CheckedStmtOp::Destructure { .. })
        ) {
            return None;
        }
        emitted
            .desugars
            .get(&node.at)
            .filter(|desugar| desugar.binder.is_none())
    }

    /// The binder and statements of the desugar of the loop target at
    /// `target`.
    fn loop_desugar(&self, target: Option<&SourceSpan>) -> Option<(&'a str, &'a [CheckedStmtId])> {
        let emitted = self.emitted;
        let desugar = emitted.desugars.get(target?)?;
        Some((desugar.binder.as_deref()?, &desugar.statements))
    }

    /// `body`'s sealed statements, inside one more function.
    fn function_body(&mut self, body: &[CheckedStmtId]) -> Vec<CheckedStmtId> {
        self.functions += 1;
        let body = self.block(body);
        self.functions -= 1;
        body
    }

    /// `target` with each declared leaf's binding, read at its occurrence
    /// in `source`.
    fn target(&self, target: &CheckedTarget, source: Option<&String>) -> CheckedTarget {
        let kind = match &target.kind {
            CheckedTargetKind::Declare { name, mode, .. } => {
                let facts = self.facts(&SourceSpan::syntax(
                    source.cloned(),
                    target.span,
                    target.syntax_id,
                ));
                CheckedTargetKind::Declare {
                    name: name.clone(),
                    mode: *mode,
                    binding: facts.binding,
                    binding_ty: facts.binding_ty.clone(),
                }
            }
            CheckedTargetKind::Tuple(items) => CheckedTargetKind::Tuple(
                items.iter().map(|item| self.target(item, source)).collect(),
            ),
            CheckedTargetKind::Sequence(items) => CheckedTargetKind::Sequence(
                items
                    .iter()
                    .map(|item| match item {
                        CheckedSequenceTarget::Element(item) => {
                            CheckedSequenceTarget::Element(self.target(item, source))
                        }
                        CheckedSequenceTarget::Star(item) => {
                            CheckedSequenceTarget::Star(self.target(item, source))
                        }
                    })
                    .collect(),
            ),
            kind @ (CheckedTargetKind::Discard | CheckedTargetKind::Place(_)) => kind.clone(),
        };
        CheckedTarget {
            kind,
            span: target.span,
            syntax_id: target.syntax_id,
        }
    }

    /// The name a bracket's written base spells, read from the base's
    /// emitted op: a spelled construction may stand in its sealed place.
    fn bracket_base(&self, op: &CheckedOp) -> Option<&'a str> {
        let object = match op {
            CheckedOp::Index { object, .. } => object,
            CheckedOp::MultiIndex { object, args }
                if args.iter().all(|argument| {
                    matches!(
                        argument,
                        crate::checked::CheckedSubscriptArg::Index(_)
                            | crate::checked::CheckedSubscriptArg::Keyword { .. }
                    )
                }) =>
            {
                object
            }
            _ => return None,
        };
        let emitted = self.emitted;
        match emitted.expressions.get(object.0 as usize)?.op.as_ref()? {
            CheckedOp::Identifier(name) => Some(name),
            _ => None,
        }
    }

    /// One case's planned commands and path reads, as emitted slots.
    fn case_program(
        &self,
        plan: Option<&MatchPlan>,
        index: usize,
    ) -> (Vec<CheckedCommand>, Vec<CheckedPathRead>) {
        let reads = plan
            .and_then(|plan| plan.reads.get(index))
            .map(|reads| {
                reads
                    .iter()
                    .map(|read| CheckedPathRead {
                        path: read.path,
                        declaration: self.planned_statement(&read.declaration),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let commands = plan
            .and_then(|plan| plan.cases.get(index))
            .map(|commands| {
                commands
                    .iter()
                    .map(|command| self.command(command))
                    .collect()
            })
            .unwrap_or_default();
        (commands, reads)
    }

    fn command(&self, planned: &PlannedCommand) -> CheckedCommand {
        match planned {
            PlannedCommand::Equal { test, reads } => CheckedCommand::Equal {
                test: self.planned_expression(test),
                reads: reads.clone(),
            },
            PlannedCommand::ListLength { test, reads } => CheckedCommand::ListLength {
                test: self.planned_expression(test),
                reads: reads.clone(),
            },
            PlannedCommand::EnumTag {
                tag,
                test,
                case,
                reads,
            } => CheckedCommand::EnumTag {
                tag: self.planned_statement(tag),
                test: self.planned_expression(test),
                case: *case,
                reads: reads.clone(),
            },
            PlannedCommand::Bind { declaration, reads } => CheckedCommand::Bind {
                declaration: self.planned_statement(declaration),
                reads: reads.clone(),
            },
            PlannedCommand::Alternatives(alternatives) => CheckedCommand::Alternatives(
                alternatives
                    .iter()
                    .map(|commands| {
                        commands
                            .iter()
                            .map(|command| self.command(command))
                            .collect()
                    })
                    .collect(),
            ),
        }
    }

    /// An expression op as its node holds it once sealed, its operands
    /// still emitted slots.
    fn shape_expression(&self, node: &EmittedExpr, op: &CheckedOp, facts: &NodeFacts) -> CheckedOp {
        match op {
            CheckedOp::Call {
                name,
                param_args,
                args,
                kwargs,
            } => CheckedOp::Call {
                name: name.clone(),
                param_args: self.parameters(node, param_args, facts),
                args: args.clone(),
                kwargs: kwargs.clone(),
            },
            CheckedOp::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } => CheckedOp::Invoke {
                callee: *callee,
                param_args: self.parameters(node, param_args, facts),
                args: args.clone(),
                kwargs: kwargs.clone(),
            },
            CheckedOp::Index { object, .. } if let Some(attribute) = subscript_attribute(facts) => {
                CheckedOp::Index {
                    object: *object,
                    index: CheckedParamArg::Attribute {
                        value: Some(attribute),
                        applications: self.outermost(node.applications.first()),
                    },
                }
            }
            CheckedOp::TypeValue { .. } => CheckedOp::TypeValue {
                ty: facts.type_operand.clone(),
                applications: self.outermost(node.applications.first()),
            },
            CheckedOp::Comprehension {
                kind,
                key,
                value,
                clauses,
            } => CheckedOp::Comprehension {
                kind: *kind,
                key: *key,
                value: *value,
                clauses: clauses
                    .iter()
                    .enumerate()
                    .map(|(index, clause)| {
                        self.clause(
                            clause,
                            node.loop_targets.get(index).and_then(Option::as_ref),
                        )
                    })
                    .collect(),
            },
            op => op.clone(),
        }
    }

    /// A call's written bracket arguments, one per parameter as emitted,
    /// regrouped as its binder laid them out, each one the check bound to
    /// an attribute that attribute.
    fn parameters(
        &self,
        node: &EmittedExpr,
        written: &[CheckedParameter],
        facts: &NodeFacts,
    ) -> Vec<CheckedParameter> {
        let written: Vec<&CheckedParamArg> = written
            .iter()
            .filter_map(|parameter| parameter.arguments.first())
            .collect();
        parameter_layout(facts.parameter_attributes.as_ref(), written.len())
            .into_iter()
            .map(|(declaration, bound)| CheckedParameter {
                declaration,
                arguments: bound
                    .into_iter()
                    .map(|(position, attribute)| {
                        self.parameter_argument(
                            written[position],
                            attribute,
                            node.applications.get(position),
                        )
                    })
                    .collect(),
            })
            .collect()
    }

    /// The written bracket argument `argument` as bound to `attribute`:
    /// a value bound to one is the attribute, holding the lambda it is or
    /// the outermost lifted applications among `candidates`.
    fn parameter_argument(
        &self,
        argument: &CheckedParamArg,
        attribute: Option<TyArg>,
        candidates: Option<&Vec<CheckedNodeId>>,
    ) -> CheckedParamArg {
        match argument {
            CheckedParamArg::Value(value) if attribute.is_some() => {
                let lambda = self
                    .emitted
                    .expressions
                    .get(value.0 as usize)
                    .is_some_and(|node| matches!(node.op, Some(CheckedOp::Lambda { .. })));
                CheckedParamArg::Attribute {
                    value: attribute,
                    applications: if lambda {
                        vec![*value]
                    } else {
                        self.outermost(candidates)
                    },
                }
            }
            CheckedParamArg::Named { name, value } => CheckedParamArg::Named {
                name: name.clone(),
                value: Box::new(self.parameter_argument(value, attribute, candidates)),
            },
            argument => argument.clone(),
        }
    }

    /// A comprehension clause whose written target `target` the check
    /// desugared binds the desugar's binder, its first statement the
    /// clause's unpacking.
    fn clause(
        &self,
        clause: &CheckedComprehensionClause,
        target: Option<&SourceSpan>,
    ) -> CheckedComprehensionClause {
        match (clause, self.loop_desugar(target)) {
            (CheckedComprehensionClause::For { iter, .. }, Some((binder, statements))) => {
                CheckedComprehensionClause::For {
                    var: binder.to_string(),
                    binding: BindingMode::Immutable,
                    iter: *iter,
                    unpack: statements
                        .first()
                        .and_then(|statement| self.unpack(*statement)),
                }
            }
            (clause, _) => clause.clone(),
        }
    }

    /// The unpacking a destructuring `statement` is, its target's leaves
    /// bound.
    fn unpack(&self, statement: CheckedStmtId) -> Option<CheckedComprehensionUnpack> {
        let node: &EmittedStmt = self.emitted.statements.get(statement.0 as usize)?;
        match node.op.as_ref()? {
            CheckedStmtOp::Destructure { target, value } => Some(CheckedComprehensionUnpack {
                target: self.target(target, node.at.source.as_ref()),
                value: *value,
            }),
            _ => None,
        }
    }

    /// A statement op other than a declaration as its node holds it once
    /// sealed, its operands still emitted slots and its bodies spliced.
    fn shape_statement(
        &self,
        node: &EmittedStmt,
        op: &CheckedStmtOp,
        facts: &NodeFacts,
    ) -> CheckedStmtOp {
        let source = node.at.source.as_ref();
        let optional =
            |body: &Option<Vec<CheckedStmtId>>| body.as_ref().map(|body| self.spliced(body));
        match op {
            CheckedStmtOp::VarDecl { name, value, .. } => CheckedStmtOp::VarDecl {
                name: name.clone(),
                applications: self.outermost(Some(&node.applications)),
                value: *value,
            },
            CheckedStmtOp::Destructure { target, value } => CheckedStmtOp::Destructure {
                target: self.target(target, source),
                value: *value,
            },
            CheckedStmtOp::Comptime {
                name,
                where_clauses,
                value,
                ..
            } => CheckedStmtOp::Comptime {
                name: name.clone(),
                where_clauses: where_clauses.clone(),
                value: *value,
                denotes: facts.comptime_value.as_deref().cloned(),
            },
            CheckedStmtOp::ComptimeAssert { cond, message, .. } => CheckedStmtOp::ComptimeAssert {
                cond: *cond,
                message: *message,
                denotes: message.and_then(|message| {
                    self.expression_facts(message)
                        .comptime_value
                        .as_deref()
                        .cloned()
                }),
                fails: self.expression_facts(*cond).failing_assertion,
            },
            CheckedStmtOp::If { branches, orelse } => CheckedStmtOp::If {
                branches: self.branches(branches, source),
                orelse: optional(orelse),
            },
            CheckedStmtOp::ComptimeIf { branches, orelse } => CheckedStmtOp::ComptimeIf {
                branches: self.branches(branches, source),
                orelse: optional(orelse),
            },
            CheckedStmtOp::While { cond, body, orelse } => CheckedStmtOp::While {
                cond: self.condition(cond, source),
                body: self.spliced(body),
                orelse: optional(orelse),
            },
            CheckedStmtOp::ComptimeFor { var, iter, body } => CheckedStmtOp::ComptimeFor {
                var: var.clone(),
                iter: *iter,
                body: self.spliced(body),
            },
            CheckedStmtOp::Match { cases, .. } => {
                let plan = facts.match_plan.as_ref();
                CheckedStmtOp::Match {
                    anchor: self.anchor(plan),
                    cases: self.cases(cases, plan),
                    emission: plan.map(|plan| plan.emission.clone()).unwrap_or_default(),
                }
            }
            CheckedStmtOp::ComptimeMatch { cases, .. } => {
                let plan = facts.match_plan.as_ref();
                CheckedStmtOp::ComptimeMatch {
                    anchor: self.anchor(plan),
                    cases: self.cases(cases, plan),
                }
            }
            CheckedStmtOp::For {
                var,
                binding,
                iter,
                body,
                orelse,
            } => match self.loop_desugar(node.loop_target.as_ref()) {
                Some((binder, statements)) => CheckedStmtOp::For {
                    var: binder.to_string(),
                    binding: BindingMode::Immutable,
                    iter: *iter,
                    body: self.spliced(&[statements, body].concat()),
                    orelse: optional(orelse),
                },
                None => CheckedStmtOp::For {
                    var: var.clone(),
                    binding: *binding,
                    iter: *iter,
                    body: self.spliced(body),
                    orelse: optional(orelse),
                },
            },
            CheckedStmtOp::With { items, body } => CheckedStmtOp::With {
                items: items.clone(),
                body: self.spliced(body),
            },
            CheckedStmtOp::Try {
                body,
                except,
                orelse,
                finalbody,
            } => CheckedStmtOp::Try {
                body: self.spliced(body),
                except: except
                    .as_ref()
                    .map(|(name, body)| (name.clone(), self.spliced(body))),
                orelse: optional(orelse),
                finalbody: optional(finalbody),
            },
            CheckedStmtOp::Scope(body) => CheckedStmtOp::Scope(self.spliced(body)),
            op => op.clone(),
        }
    }

    fn expression_facts(&self, slot: CheckedNodeId) -> &'a NodeFacts {
        let emitted = self.emitted;
        emitted
            .expressions
            .get(slot.0 as usize)
            .map_or(&NodeFacts::EMPTY, |node| held(node.facts.as_deref()))
    }

    fn anchor(&self, plan: Option<&MatchPlan>) -> Option<CheckedStmtId> {
        plan.and_then(|plan| plan.anchor.as_ref())
            .map(|anchor| self.planned_statement(anchor))
    }

    fn branches(
        &self,
        branches: &[(CheckedCondition, Vec<CheckedStmtId>)],
        source: Option<&String>,
    ) -> Vec<(CheckedCondition, Vec<CheckedStmtId>)> {
        branches
            .iter()
            .map(|(condition, body)| (self.condition(condition, source), self.spliced(body)))
            .collect()
    }

    /// A `case` condition with the plan the check recorded at its pattern's
    /// occurrence in `source`.
    fn condition(&self, condition: &CheckedCondition, source: Option<&String>) -> CheckedCondition {
        match condition {
            CheckedCondition::Case { pattern, value, .. } => {
                let plan = self
                    .facts(&SourceSpan::syntax(
                        source.cloned(),
                        pattern.span,
                        pattern.syntax_id,
                    ))
                    .match_plan
                    .as_ref();
                let (commands, reads) = self.case_program(plan, 0);
                CheckedCondition::Case {
                    pattern: pattern.clone(),
                    value: *value,
                    anchor: self.anchor(plan),
                    commands,
                    reads,
                }
            }
            condition @ CheckedCondition::Expr(_) => condition.clone(),
        }
    }

    /// A match's cases with its `plan`: a `comptime match` case's guard is
    /// a conjunct of its condition, which holds its own copy of it.
    fn cases(&self, cases: &[CheckedMatchCase], plan: Option<&MatchPlan>) -> Vec<CheckedMatchCase> {
        let comptime = plan.is_some_and(|plan| !plan.conditions.is_empty());
        cases
            .iter()
            .enumerate()
            .map(|(index, case)| {
                let (commands, reads) = self.case_program(plan, index);
                CheckedMatchCase {
                    pattern: case.pattern.clone(),
                    commands,
                    reads,
                    guard: case.guard.filter(|_| !comptime),
                    condition: plan
                        .and_then(|plan| plan.conditions.get(index))
                        .and_then(Option::as_ref)
                        .map(|condition| self.planned_expression(condition)),
                    body: self.spliced(&case.body),
                    unreachable: plan
                        .and_then(|plan| plan.unreachable.get(index).copied())
                        .unwrap_or(false),
                }
            })
            .collect()
    }

    /// A `def` declared at `at` with identity `syntax`: its symbol and
    /// signature from its sites, its captures and storage from its `facts`.
    fn def(
        &mut self,
        at: &SourceSpan,
        syntax: SyntaxId,
        def: &CheckedDef,
        facts: &NodeFacts,
    ) -> CheckedDef {
        let symbol = self.signatures.function_symbol(
            self.functions,
            at.source.as_ref(),
            at.span,
            syntax,
            &def.name,
        );
        let signature =
            self.signatures
                .function(at.source.as_ref(), at.span, syntax, def.params.len());
        let params = map_params(self, &def.params);
        let where_clauses = self.expressions(&def.where_clauses);
        let body = self.function_body(&def.body);
        CheckedDef {
            symbol,
            signature,
            params,
            where_clauses,
            body,
            captures: facts.captures.clone().unwrap_or_default(),
            storage: facts.closure_storage.as_deref().cloned(),
            ..def.clone()
        }
    }

    fn expressions(&mut self, values: &[CheckedNodeId]) -> Vec<CheckedNodeId> {
        values.iter().map(|value| self.expression(*value)).collect()
    }

    /// A struct declared at `at`: its methods' symbols and signatures and
    /// its fields' types and conventions from its sites.
    fn structure(&mut self, at: &SourceSpan, declaration: &CheckedStruct) -> CheckedStruct {
        let name = &declaration.name;
        let conformance_conditions = declaration
            .conformance_conditions
            .iter()
            .map(|(entry, value)| (entry.clone(), self.expression(*value)))
            .collect();
        let where_clauses = self.expressions(&declaration.where_clauses);
        let associated = declaration
            .associated
            .iter()
            .map(|member| CheckedStructComptime {
                name: member.name.clone(),
                where_clauses: self.expressions(&member.where_clauses),
                value: self.expression(member.value),
            })
            .collect();
        let mut methods = Vec::with_capacity(declaration.methods.len());
        for (position, method) in declaration.methods.iter().enumerate() {
            let symbol = self.signatures.method_symbol(
                at.source.as_ref(),
                name,
                position,
                &method.registered,
            );
            let signature =
                self.signatures
                    .method(at.source.as_ref(), name, position, method.params.len());
            let params = map_params(self, &method.params);
            let where_clauses = self.expressions(&method.where_clauses);
            let body = self.function_body(&method.body);
            methods.push(CheckedMethod {
                symbol,
                signature,
                params,
                where_clauses,
                body,
                ..method.clone()
            });
        }
        let fields = self.signatures.fields(
            at.source.as_ref(),
            name,
            declaration
                .fields
                .iter()
                .map(|field| (field.name.clone(), field.reference)),
        );
        CheckedStruct {
            conformance_conditions,
            where_clauses,
            fields,
            associated,
            methods,
            ..declaration.clone()
        }
    }

    /// A trait declared at `at`: its methods' symbols and signatures from
    /// its sites.
    fn trait_declaration(&mut self, at: &SourceSpan, declaration: &CheckedTrait) -> CheckedTrait {
        let name = &declaration.name;
        let mut methods = Vec::with_capacity(declaration.methods.len());
        for (index, method) in declaration.methods.iter().enumerate() {
            let symbol =
                self.signatures
                    .method_symbol(at.source.as_ref(), name, index, &method.name);
            let signature =
                self.signatures
                    .method(at.source.as_ref(), name, index, method.params.len());
            let params = map_params(self, &method.params);
            let where_clauses = self.expressions(&method.where_clauses);
            let default = method.default.as_ref().map(|default| CheckedTraitDefault {
                body: self.function_body(&default.body),
                inline: default.inline,
            });
            methods.push(CheckedTraitMethod {
                symbol,
                signature,
                params,
                where_clauses,
                default,
                ..method.clone()
            });
        }
        let comptime_members = declaration
            .comptime_members
            .iter()
            .map(|member| CheckedTraitComptime {
                name: member.name.clone(),
                where_clauses: self.expressions(&member.where_clauses),
            })
            .collect();
        CheckedTrait {
            methods,
            comptime_members,
            ..declaration.clone()
        }
    }

    /// The sealed nodes of a root list's `statements`.
    fn roots(&mut self, statements: &[Stmt]) -> Vec<CheckedStmtId> {
        let slots: Vec<CheckedStmtId> = statements
            .iter()
            .filter_map(|statement| {
                let slot = self.planned_statement(statement);
                if slot.0 == u32::MAX {
                    self.unsealed.borrow_mut().count += 1;
                    None
                } else {
                    Some(slot)
                }
            })
            .collect();
        self.block(&slots)
    }

    /// A placeholder for an expression no emitted node or op holds, at
    /// `at` where the seal knows it: the walk stays total, and the tables
    /// are rejected.
    fn missing_expression(&mut self, at: Option<&SourceSpan>) -> CheckedNodeId {
        self.unsealed.get_mut().miss(at);
        let id = CheckedNodeId(self.nodes.len() as u32);
        self.nodes.push(expression_node(
            id,
            CheckedOp::None,
            (Span::default(), None, SyntaxId(0)),
            &NodeFacts::EMPTY,
            None,
        ));
        id
    }

    /// As [`Self::missing_expression`], for a statement.
    fn missing_statement(&mut self, at: Option<&SourceSpan>) -> CheckedStmtId {
        self.unsealed.get_mut().miss(at);
        let id = CheckedStmtId(self.statements.len() as u32);
        self.statements.push(statement_node(
            id,
            CheckedStmtOp::Pass,
            (Span::default(), None, SyntaxId(0)),
            &NodeFacts::EMPTY,
        ));
        id
    }
}

/// A node's facts, or none recorded.
fn held(facts: Option<&NodeFacts>) -> &NodeFacts {
    facts.unwrap_or(&NodeFacts::EMPTY)
}

#[cfg(test)]
mod tests {
    use super::seal;
    use crate::checked::{
        BracketBinding, BracketBound, CheckedOp, CheckedParamArg, CheckedParameter, CheckedStmtOp,
        CheckedTables, ConformanceFacts, DeclarationSites, NodeFacts, SemanticAdjustment,
        SpelledConstruction,
    };
    use crate::emitted::Unsealed;
    use crate::emitted::{EmittedDesugar, EmittedExpr, EmittedNodes, EmittedSlot, EmittedStmt};
    use mojito_ast::ast::{Expr, ExprKind, ParamArg, Stmt, StmtKind};
    use mojito_common::token::{SourceSpan, SyntaxId};
    use mojito_types::types::{Ty, TyArg};
    use std::collections::HashMap;

    /// Syntax whose every node has an occurrence of its own.
    #[derive(Default)]
    struct Syntax(usize);

    impl Syntax {
        fn expr(&mut self, kind: ExprKind) -> Expr {
            self.0 += 1;
            let mut expression = Expr::new(kind, (self.0, self.0 + 1));
            expression.syntax_id = SyntaxId(self.0 as u64);
            expression
        }

        fn stmt(&mut self, kind: StmtKind) -> Stmt {
            self.0 += 1;
            let mut statement = Stmt::new(kind, (self.0, self.0 + 1));
            statement.syntax_id = SyntaxId(self.0 as u64);
            statement
        }
    }

    /// Emitted nodes allocated one at a time, each indexed by its syntax.
    #[derive(Default)]
    struct Emitted(EmittedNodes);

    impl Emitted {
        fn expr(
            &mut self,
            syntax: &Expr,
            op: CheckedOp,
            applications: Vec<Vec<crate::checked::CheckedNodeId>>,
        ) -> crate::checked::CheckedNodeId {
            let id = crate::checked::CheckedNodeId(self.0.expressions.len() as u32);
            self.0.expressions.push(EmittedExpr {
                at: syntax.source_span(),
                op: Some(op),
                applications,
                loop_targets: Vec::new(),
                facts: None,
            });
            self.0
                .index
                .insert(syntax.source_span(), EmittedSlot::Expression(id));
            id
        }

        fn stmt(&mut self, syntax: &Stmt, op: CheckedStmtOp) -> crate::checked::CheckedStmtId {
            let id = crate::checked::CheckedStmtId(self.0.statements.len() as u32);
            self.0.statements.push(EmittedStmt {
                at: syntax.source_span(),
                op: Some(op),
                applications: Vec::new(),
                loop_target: None,
                facts: None,
            });
            self.0
                .index
                .insert(syntax.source_span(), EmittedSlot::Statement(id));
            id
        }
    }

    /// The tables sealed from `emitted` for the roots `statements`, with
    /// each of `facts` on the node its occurrence's index names, or else
    /// detached.
    fn sealed(
        emitted: &EmittedNodes,
        statements: &[Stmt],
        facts: &HashMap<SourceSpan, NodeFacts>,
    ) -> Result<CheckedTables, Unsealed> {
        let mut emitted = emitted.clone();
        for (at, record) in facts {
            let held = match emitted.index.get(at) {
                Some(EmittedSlot::Expression(id)) => &mut emitted.expressions[id.0 as usize].facts,
                Some(EmittedSlot::Statement(id)) => &mut emitted.statements[id.0 as usize].facts,
                None => {
                    emitted.detached.insert(at.clone(), record.clone());
                    continue;
                }
            };
            *held = Some(Box::new(record.clone()));
        }
        seal(
            &emitted,
            statements,
            &mut [],
            &DeclarationSites::default(),
            &ConformanceFacts::default(),
        )
    }

    fn written(slot: crate::checked::CheckedNodeId) -> CheckedParameter {
        CheckedParameter {
            declaration: None,
            arguments: vec![CheckedParamArg::Value(slot)],
        }
    }

    #[test]
    fn a_call_seals_its_bracket_arguments_in_declaration_order() {
        let mut syntax = Syntax::default();
        let (a, b, y) = (
            syntax.expr(ExprKind::Identifier("a".to_string())),
            syntax.expr(ExprKind::Identifier("b".to_string())),
            syntax.expr(ExprKind::Identifier("y".to_string())),
        );
        let call = syntax.expr(ExprKind::Call {
            name: "f".to_string(),
            param_args: vec![ParamArg::Value(a.clone()), ParamArg::Value(b.clone())],
            args: vec![y.clone()],
            kwargs: Vec::new(),
        });
        let statement = syntax.stmt(StmtKind::Expr(call.clone()));
        let mut emitted = Emitted::default();
        let (a_slot, b_slot, y_slot) = (
            emitted.expr(&a, CheckedOp::Identifier("a".to_string()), Vec::new()),
            emitted.expr(&b, CheckedOp::Identifier("b".to_string()), Vec::new()),
            emitted.expr(&y, CheckedOp::Identifier("y".to_string()), Vec::new()),
        );
        let call_slot = emitted.expr(
            &call,
            CheckedOp::Call {
                name: "f".to_string(),
                param_args: vec![written(a_slot), written(b_slot)],
                args: vec![y_slot],
                kwargs: Vec::new(),
            },
            vec![vec![a_slot], vec![b_slot]],
        );
        emitted.stmt(&statement, CheckedStmtOp::Expr(call_slot));
        let facts = HashMap::from([(
            call.source_span(),
            NodeFacts {
                parameter_attributes: Some(BracketBinding {
                    slots: vec![
                        vec![BracketBound {
                            position: 1,
                            attribute: Some(TyArg::Ty(Ty::int())),
                        }],
                        vec![BracketBound {
                            position: 0,
                            attribute: None,
                        }],
                    ],
                    declarations: vec![Some("T".to_string()), Some("n".to_string())],
                }),
                ..NodeFacts::default()
            },
        )]);
        let sealed = sealed(&emitted.0, &[statement], &facts).expect("every node was emitted");
        assert_eq!(
            sealed.expressions.len(),
            3,
            "the attribute's value has no node"
        );
        assert_eq!(sealed.statements.len(), 1);
        assert_eq!(sealed.roots(), &[crate::checked::CheckedStmtId(0)]);
        let Some(CheckedOp::Call {
            param_args, args, ..
        }) = sealed.expressions.last().map(|node| &node.op)
        else {
            unreachable!("the root expression is the call")
        };
        assert_eq!(
            args,
            &[crate::checked::CheckedNodeId(1)],
            "the value argument is sealed after the bracket value it follows"
        );
        let declarations: Vec<_> = param_args
            .iter()
            .map(|parameter| parameter.declaration.as_deref())
            .collect();
        assert_eq!(declarations, [Some("T"), Some("n")]);
        assert!(matches!(
            param_args[0].arguments.as_slice(),
            [CheckedParamArg::Attribute { value: Some(_), applications }] if applications.is_empty()
        ));
        assert!(matches!(
            param_args[1].arguments.as_slice(),
            [CheckedParamArg::Value(crate::checked::CheckedNodeId(0))]
        ));
        assert!(matches!(
            &sealed.expressions[0].op,
            CheckedOp::Identifier(name) if name == "a"
        ));
    }

    #[test]
    fn a_with_statement_splices_its_desugar_and_a_nested_one() {
        let mut syntax = Syntax::default();
        let (first, second, third) = (
            syntax.stmt(StmtKind::Pass),
            syntax.stmt(StmtKind::Pass),
            syntax.stmt(StmtKind::Pass),
        );
        let with = |syntax: &mut Syntax| {
            syntax.stmt(StmtKind::With {
                items: Vec::new(),
                body: Vec::new(),
            })
        };
        let (inner, outer) = (with(&mut syntax), with(&mut syntax));
        let mut scope = syntax.stmt(StmtKind::Scope(vec![outer.clone()]));
        let mut emitted = Emitted::default();
        let slots: Vec<_> = [&first, &second, &third]
            .into_iter()
            .map(|statement| emitted.stmt(statement, CheckedStmtOp::Pass))
            .collect();
        let empty_with = || CheckedStmtOp::With {
            items: Vec::new(),
            body: Vec::new(),
        };
        let inner_slot = emitted.stmt(&inner, empty_with());
        let outer_slot = emitted.stmt(&outer, empty_with());
        emitted.stmt(&scope, CheckedStmtOp::Scope(vec![outer_slot]));
        emitted.0.desugars.insert(
            outer.source_span(),
            EmittedDesugar {
                statements: vec![slots[0], inner_slot],
                binder: None,
            },
        );
        emitted.0.desugars.insert(
            inner.source_span(),
            EmittedDesugar {
                statements: vec![slots[1], slots[2]],
                binder: None,
            },
        );
        scope.kind = StmtKind::Scope(vec![first, second, third]);
        let sealed = sealed(&emitted.0, &[scope], &HashMap::new()).expect("every node was emitted");
        assert_eq!(
            sealed.statements.len(),
            4,
            "the `with` statements are replaced"
        );
        let Some(CheckedStmtOp::Scope(body)) =
            sealed.statements.last().map(|statement| &statement.op)
        else {
            unreachable!("the root statement is the scope")
        };
        assert_eq!(body.len(), 3);
        for (statement, written) in body.iter().zip([1, 2, 3]) {
            let node = sealed.statement(*statement).expect("a sealed statement");
            assert!(matches!(node.op, CheckedStmtOp::Pass));
            assert_eq!(node.span, (written, written + 1));
        }
    }

    #[test]
    fn a_spelled_sugar_seals_as_its_construction_with_the_sugars_conversion() {
        let mut syntax = Syntax::default();
        let construction = syntax.expr(ExprKind::Identifier("x".to_string()));
        let sugar = syntax.expr(ExprKind::Int(5u64.into()));
        let statement = syntax.stmt(StmtKind::Expr(sugar.clone()));
        let mut emitted = Emitted::default();
        let sugar_slot = emitted.expr(&sugar, CheckedOp::Int(5u64.into()), Vec::new());
        emitted.expr(
            &construction,
            CheckedOp::Identifier("x".to_string()),
            Vec::new(),
        );
        emitted.stmt(&statement, CheckedStmtOp::Expr(sugar_slot));
        let facts = HashMap::from([
            (
                sugar.source_span(),
                NodeFacts {
                    adjustment: Some(Box::new(SemanticAdjustment::SpelledConstruction {
                        construction: SpelledConstruction(Box::new(construction.clone())),
                    })),
                    implicit_conversion: Some("convert".to_string()),
                    ..NodeFacts::default()
                },
            ),
            (
                construction.source_span(),
                NodeFacts {
                    ty: Some(Ty::int()),
                    ..NodeFacts::default()
                },
            ),
        ]);
        let sealed = sealed(&emitted.0, &[statement], &facts).expect("every node was emitted");
        assert_eq!(sealed.expressions.len(), 1);
        assert!(matches!(&sealed.expressions[0].op, CheckedOp::Identifier(name) if name == "x"));
        assert_eq!(sealed.expressions[0].span, construction.span);
        assert!(matches!(
            sealed.statements.last().map(|statement| &statement.op),
            Some(CheckedStmtOp::Expr(crate::checked::CheckedNodeId(0)))
        ));
        assert!(
            sealed.expressions[0]
                .adjustments
                .iter()
                .any(|adjustment| matches!(adjustment, SemanticAdjustment::ImplicitConversion(_)))
        );
    }

    #[test]
    fn a_root_with_no_emitted_node_is_unsealed() {
        let statement = Syntax::default().stmt(StmtKind::Pass);
        let Err(unsealed) = sealed(
            &EmittedNodes::default(),
            &[statement.clone()],
            &HashMap::new(),
        ) else {
            unreachable!("a root with no node cannot seal")
        };
        assert_eq!(unsealed.count, 1);
        assert_eq!(unsealed.first, Some(statement.source_span()));
    }
}

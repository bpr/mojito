//! Certificate grammar for operators over closed and parameter-typed
//! operands.

use super::{BodyShape, comparison, fact_at, operator_dispatch};
use mojito_ast::ast::{Expr, ExprKind};
use mojito_checked::templates::MethodFeatures;
use mojito_types::types::Ty;

impl BodyShape<'_> {
    /// The result of an admitted operator ([`Self::operator`]) whose operands
    /// are not closed scalars: a temporary of the operand's own type, which an
    /// instance's dunder yields exactly as a sibling call's result does.
    pub(super) fn operator_value(&self, expr: &Expr) -> bool {
        matches!(&expr.kind, ExprKind::Infix(op, left, right)
            if self.operator(expr, *op, left, right))
    }

    /// An operator over two operands of one closed struct type, or such an
    /// operand and a string literal converted to it
    /// (`String("bad: ") + name + " n: "`): a temporary of that type.
    ///
    /// The operand type's own dunder answers, which no instance changes, so
    /// the template recorded nothing at the operator, and a literal operand's
    /// conversion is selected again at the same closed types. An operand is
    /// a named place, read where it lies, or a whole value.
    pub(super) fn closed_operator(&self, expr: &Expr) -> bool {
        let ExprKind::Infix(op, left, right) = &expr.kind else {
            return false;
        };
        let operand = |operand: &Expr| {
            matches!(operand.kind, ExprKind::Str(_))
                || matches!(&operand.kind, ExprKind::Identifier(name)
                    if self.declared(name) || self.params.contains(&name.as_str()))
                || self.receiver_field(operand)
                || self.whole_value(operand)
        };
        let id = self.occurrence(expr);
        let admitted = !self.keyed
            && operator_dispatch(*op)
            && operand(left)
            && operand(right)
            && !matches!(left.kind, ExprKind::Str(_))
            && self.facts.is_none_or(|facts| {
                let at = |operand: &Expr| self.occurrence(operand);
                // A nominal-string wrap is its producer's own conversion
                // ([`Self::stringify`]), not one of the operand.
                let unconverted = |operand: &Expr| {
                    fact_at(&facts.conversions, at(operand))
                        .is_none_or(|conversion| conversion.result.is_none())
                };
                let Some(ty @ Ty::Struct(..)) = fact_at(&facts.expression_types, at(left)) else {
                    return false;
                };
                let right_typed = match &right.kind {
                    ExprKind::Str(_) => fact_at(&facts.conversions, at(right))
                        .is_some_and(|conversion| conversion.result.as_ref() == Some(ty)),
                    _ => {
                        fact_at(&facts.expression_types, at(right)) == Some(ty)
                            && unconverted(right)
                    }
                };
                !mojito_types::types::is_symbolic(ty)
                    && right_typed
                    && fact_at(&facts.expression_types, id) == Some(ty)
                    && unconverted(left)
                    && fact_at(&facts.overload_targets, id).is_none()
                    && fact_at(&facts.operation_adjustments, id).is_none()
                    && fact_at(&facts.selected_calls, id).is_none()
                    && fact_at(&facts.call_parameters, id).is_none()
            });
        admitted && self.holds(MethodFeatures::CLOSED_OPERATORS)
    }

    /// An operator over two operands of one type that mentions a struct
    /// parameter, which dispatches on the type alone, or over such an operand
    /// and a literal the dunder of a struct built over the parameter accepts.
    ///
    /// An operand is a place, a call result, or another admitted operator.
    /// The template, whose type is symbolic, recorded nothing at the
    /// operator: a bound (or a `where` assumption) proves it, or the dunder
    /// the template dispatched on a struct built over the parameter answers,
    /// with a literal operand converted into its parameter type where it
    /// must be. An instance decides the same operator on its substituted
    /// types ([`Checker::realize_operator`]). Neither check records anything
    /// at a temporary operand: a place is read where it lies, and a call
    /// result or an operator's value is moved into the dunder, or dropped
    /// after it.
    ///
    /// A closed left operand beside such a struct — a literal, or a closed
    /// scalar with no dunder for the pair — dispatches the struct's
    /// reflected dunder (`1 + self.bag` → `self.bag.__radd__(1)`), whose
    /// target and `ReflectedOperator` adjustment the template recorded at
    /// the operator; an instance names the target again at its own types.
    ///
    /// The result is the operand's own type for an arithmetic, bitwise, or
    /// shift operator, so it is not a scalar under every instance;
    /// [`Self::operator_value`] is what admits it where a temporary may go.
    pub(super) fn operator(
        &self,
        expr: &Expr,
        op: mojito_ast::ast::InfixOp,
        left: &Expr,
        right: &Expr,
    ) -> bool {
        let operand = |operand: &Expr| match &operand.kind {
            ExprKind::Identifier(name) => {
                self.params.contains(&name.as_str())
                    || self.local_kind(name).is_some()
                    || self.receiver_itself(operand)
            }
            ExprKind::Member { .. } => {
                self.receiver_field(operand) || self.reference_member(operand)
            }
            ExprKind::Index { .. } => self.slot(operand) || self.pack_element(operand),
            ExprKind::MethodCall { .. } | ExprKind::Invoke { .. } => self.call_result(operand),
            ExprKind::Infix(..) => self.operator_value(operand),
            _ => false,
        };
        let literal = |operand: &Expr| {
            matches!(
                operand.kind,
                ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Str(_) | ExprKind::Bool(_)
            )
        };
        let right_literal = literal(right);
        let id = self.occurrence(expr);
        let admitted = !self.keyed
            && operator_dispatch(op)
            && (literal(left) || operand(left))
            && (right_literal || operand(right))
            && !(literal(left) && right_literal)
            && self.facts.is_none_or(|facts| {
                let targeted = facts.overload_targets.iter().any(|(site, _)| *site == id);
                let adjustments: Vec<_> = facts
                    .operation_adjustments
                    .iter()
                    .filter(|(site, _)| *site == id)
                    .map(|(_, adjustment)| adjustment)
                    .collect();
                let reflected = matches!(
                    adjustments.as_slice(),
                    [mojito_checked::checked::SemanticAdjustment::ReflectedOperator]
                );
                let typed = fact_at(&facts.expression_types, self.occurrence(left))
                    .zip(fact_at(&facts.expression_types, self.occurrence(right)))
                    .is_some_and(|(left, right)| {
                        if reflected {
                            !mojito_types::types::is_symbolic(left)
                                && mojito_types::types::is_symbolic(right)
                                && matches!(right, Ty::Struct(..))
                        } else {
                            // A comparison of a symbolic lane's values is
                            // native where the lane folds to a scalar, and a
                            // value no dunder types ([`Self::lane_comparison`]).
                            mojito_types::types::is_symbolic(left)
                                && !(comparison(op) && self.value_shaped_scalar(left))
                                && (left == right
                                    || (right_literal && matches!(left, Ty::Struct(..))))
                        }
                    });
                typed && (targeted == reflected) && (reflected || adjustments.is_empty())
            });
        if admitted {
            let mut operators = self.operators.borrow_mut();
            if !operators.contains(&id) {
                operators.push(id);
            }
        }
        admitted && self.holds(MethodFeatures::OPERATOR_DISPATCH)
    }
}

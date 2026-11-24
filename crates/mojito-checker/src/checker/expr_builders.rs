//! Expression builders for the checker's unit tests of predicates that read
//! an annotation or `comptime` body off its expression.

use mojito_ast::ast::{Expr, ExprKind, InfixOp, SubscriptArg};

pub(super) fn ident(name: &str) -> Expr {
    Expr::from(ExprKind::Identifier(name.into()))
}

pub(super) fn int(n: i64) -> Expr {
    Expr::from(ExprKind::Int(n.into()))
}

pub(super) fn member(object: Expr, field: &str) -> Expr {
    Expr::from(ExprKind::Member {
        object: Box::new(object),
        field: field.into(),
    })
}

pub(super) fn index(object: Expr, index: Expr) -> Expr {
    Expr::from(ExprKind::Index {
        object: Box::new(object),
        index: Box::new(index),
    })
}

pub(super) fn multi_index(object: Expr, operands: Vec<Expr>) -> Expr {
    Expr::from(ExprKind::MultiIndex {
        object: Box::new(object),
        args: operands.into_iter().map(SubscriptArg::Index).collect(),
    })
}

pub(super) fn call(name: &str, args: Vec<Expr>) -> Expr {
    Expr::from(ExprKind::Call {
        name: name.into(),
        param_args: Vec::new(),
        args,
        kwargs: Vec::new(),
    })
}

pub(super) fn spread(operand: Expr) -> Expr {
    Expr::from(ExprKind::Spread(Box::new(operand)))
}

pub(super) fn add(left: Expr, right: Expr) -> Expr {
    Expr::from(ExprKind::Infix(
        InfixOp::Add,
        Box::new(left),
        Box::new(right),
    ))
}

pub(super) fn conditional(then_branch: Expr, else_branch: Expr) -> Expr {
    Expr::from(ExprKind::IfExpr {
        cond: Box::new(ident("c")),
        then_branch: Box::new(then_branch),
        else_branch: Box::new(else_branch),
    })
}

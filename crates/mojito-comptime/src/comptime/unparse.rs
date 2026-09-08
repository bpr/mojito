//! Source spelling of a `where` clause for diagnostics: upstream's
//! `constraint declared here evaluated to False, expected '<clause>'` note
//! prints the clause as declared, with the enclosing struct's parameters
//! spelled bare (`conforms_to(T, Copyable)`, `Ts.all_conforms_to[Deinitable]()`).

use super::*;
use mojito_ast::ast::{InfixOp, PrefixOp};

/// Upstream's wording for a failed message-less `where` clause.
pub(super) fn violated_constraint_message(clause: &Expr) -> String {
    format!(
        "violated constraint; constraint declared here evaluated to False, expected '{}'",
        render_where_clause(clause)
    )
}

/// The clause as written, `Self.` dropped.
pub(super) fn render_where_clause(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Int(value) => value.to_string(),
        ExprKind::Float(value) => value.to_string(),
        ExprKind::Bool(value) => if *value { "True" } else { "False" }.to_string(),
        ExprKind::Str(value) => format!("{value:?}"),
        ExprKind::None => "None".to_string(),
        ExprKind::Identifier(name) => name.clone(),
        ExprKind::TypeValue(ty) => render_type(ty),
        ExprKind::Member { object, field } => match &object.kind {
            ExprKind::Identifier(name) if name == "Self" => field.clone(),
            _ => format!("{}.{field}", render_where_clause(object)),
        },
        ExprKind::Call {
            name,
            param_args,
            args,
            ..
        } => format!(
            "{name}{}({})",
            render_param_args(param_args),
            render_args(args)
        ),
        ExprKind::Invoke {
            callee,
            param_args,
            args,
            ..
        } => format!(
            "{}{}({})",
            render_where_clause(callee),
            render_param_args(param_args),
            render_args(args)
        ),
        ExprKind::MethodCall {
            object,
            method,
            args,
            ..
        } => format!(
            "{}.{method}({})",
            render_where_clause(object),
            render_args(args)
        ),
        ExprKind::TypeApply { name, args } => format!("{name}{}", render_param_args(args)),
        ExprKind::Index { object, index } => format!(
            "{}[{}]",
            render_where_clause(object),
            render_where_clause(index)
        ),
        ExprKind::Prefix(op, inner) => {
            let inner = render_where_clause(inner);
            match op {
                PrefixOp::Not => format!("not {inner}"),
                PrefixOp::Neg => format!("-{inner}"),
                PrefixOp::Invert => format!("~{inner}"),
            }
        }
        ExprKind::Infix(op, left, right) => format!(
            "{} {} {}",
            render_where_clause(left),
            infix_spelling(*op),
            render_where_clause(right)
        ),
        ExprKind::Compare { first, rest } => {
            let mut rendered = render_where_clause(first);
            for (op, operand) in rest {
                rendered.push(' ');
                rendered.push_str(infix_spelling(*op));
                rendered.push(' ');
                rendered.push_str(&render_where_clause(operand));
            }
            rendered
        }
        ExprKind::TupleLit(items) => format!("({})", render_args(items)),
        _ => "<expr>".to_string(),
    }
}

fn render_args(args: &[Expr]) -> String {
    args.iter()
        .map(render_where_clause)
        .collect::<Vec<_>>()
        .join(", ")
}

/// `[a, b]`, or nothing for an empty list.
fn render_param_args(args: &[ParamArg]) -> String {
    if args.is_empty() {
        return String::new();
    }
    format!(
        "[{}]",
        args.iter()
            .map(render_param_arg)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn render_param_arg(arg: &ParamArg) -> String {
    match arg {
        ParamArg::Type(ty) => render_type(ty),
        ParamArg::Value(value) => render_where_clause(value),
        ParamArg::Named { name, value } => format!("{name}={}", render_param_arg(value)),
    }
}

fn render_type(ty: &Type) -> String {
    match ty {
        Type::Int => "Int".to_string(),
        Type::UInt => "UInt".to_string(),
        Type::Bool => "Bool".to_string(),
        Type::StringLiteral => "StringLiteral".to_string(),
        Type::Float64 => "Float64".to_string(),
        Type::None => "None".to_string(),
        Type::Named(name, args) => format!("{name}{}", render_param_args(args)),
        Type::SelfParam(name) => name.clone(),
        Type::SelfType => "Self".to_string(),
        Type::Assoc { base, name, args } => {
            format!("{}.{name}{}", render_type(base), render_param_args(args))
        }
        _ => "<type>".to_string(),
    }
}

fn infix_spelling(op: InfixOp) -> &'static str {
    match op {
        InfixOp::Add => "+",
        InfixOp::Sub => "-",
        InfixOp::Mul => "*",
        InfixOp::Div => "/",
        InfixOp::FloorDiv => "//",
        InfixOp::Mod => "%",
        InfixOp::MatMul => "@",
        InfixOp::Shl => "<<",
        InfixOp::Shr => ">>",
        InfixOp::BitAnd => "&",
        InfixOp::BitOr => "|",
        InfixOp::BitXor => "^",
        InfixOp::Pow => "**",
        InfixOp::Eq => "==",
        InfixOp::Ne => "!=",
        InfixOp::Lt => "<",
        InfixOp::Gt => ">",
        InfixOp::Le => "<=",
        InfixOp::Ge => ">=",
        InfixOp::And => "and",
        InfixOp::Or => "or",
        InfixOp::In => "in",
        InfixOp::NotIn => "not in",
        InfixOp::Is => "is",
        InfixOp::IsNot => "is not",
    }
}

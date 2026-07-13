//! Checked semantic handoff between the frontend and lowering.

use crate::ast::{Expr, ExprKind, PrefixOp};
use crate::ast::{Stmt, Type};
use crate::token::Span;
use crate::types::Ty;
use std::collections::HashMap;

/// A successfully checked program plus semantic facts that downstream phases
/// previously recomputed from AST syntax or checker-private side tables.
#[derive(Debug, Clone)]
pub struct CheckedProgram {
    statements: Vec<Stmt>,
    overload_targets: HashMap<Span, String>,
    resolved_types: Vec<(Type, Ty)>,
}

#[derive(Debug, Clone)]
pub enum CheckedConst {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    None,
}

impl CheckedConst {
    pub fn from_expr(expr: &Expr) -> Option<Self> {
        match &expr.kind {
            ExprKind::Int(value) => Some(Self::Int(*value)),
            ExprKind::Float(value) => Some(Self::Float(*value)),
            ExprKind::Bool(value) => Some(Self::Bool(*value)),
            ExprKind::Str(value) => Some(Self::String(value.clone())),
            ExprKind::None => Some(Self::None),
            ExprKind::Prefix(PrefixOp::Neg, inner) => match Self::from_expr(inner)? {
                Self::Int(value) => Some(Self::Int(-value)),
                Self::Float(value) => Some(Self::Float(-value)),
                _ => None,
            },
            _ => None,
        }
    }
}

impl CheckedProgram {
    pub(crate) fn new(
        statements: Vec<Stmt>,
        overload_targets: HashMap<Span, String>,
        resolved_types: Vec<(Type, Ty)>,
    ) -> Self {
        Self {
            statements,
            overload_targets,
            resolved_types,
        }
    }

    pub fn statements(&self) -> &[Stmt] {
        &self.statements
    }

    pub fn overload_targets(&self) -> &HashMap<Span, String> {
        &self.overload_targets
    }

    /// Resolve an annotation exactly as the checker did. Equal annotations in a
    /// checked program necessarily resolve consistently in their declaration
    /// context for the currently supported type syntax.
    pub fn resolved_type(&self, annotation: &Type) -> Option<&Ty> {
        self.resolved_types
            .iter()
            .rev()
            .find(|(source, _)| source == annotation)
            .map(|(_, ty)| ty)
    }

    pub fn declaration_module(&self, span: Span) -> Option<&str> {
        self.statements
            .iter()
            .find(|stmt| stmt.span == span)
            .and_then(|stmt| stmt.module.as_deref())
    }

    /// Compatibility snapshot for internally generated CTFE programs that were
    /// already proven safe before specialization but intentionally omit some
    /// source declarations. Production compilation must use `check_program`.
    pub(crate) fn unchecked(statements: &[Stmt]) -> Self {
        let mut resolved_types = Vec::new();
        collect_annotations(statements, &mut resolved_types);
        Self::new(statements.to_vec(), HashMap::new(), resolved_types)
    }
}

fn collect_annotations(statements: &[Stmt], out: &mut Vec<(Type, Ty)>) {
    for statement in statements {
        match &statement.kind {
            crate::ast::StmtKind::Def { params, body, .. } => {
                for param in params {
                    out.push((param.ty.clone(), unchecked_ty(&param.ty)));
                }
                collect_annotations(body, out);
            }
            crate::ast::StmtKind::Struct {
                fields, methods, ..
            } => {
                for field in fields {
                    out.push((field.ty.clone(), unchecked_ty(&field.ty)));
                }
                for method in methods {
                    for param in &method.params {
                        out.push((param.ty.clone(), unchecked_ty(&param.ty)));
                    }
                    collect_annotations(&method.body, out);
                }
            }
            _ => {}
        }
    }
}

fn unchecked_ty(ty: &Type) -> Ty {
    match ty {
        Type::Int => Ty::Int,
        Type::UInt => Ty::UInt,
        Type::Bool => Ty::Bool,
        Type::String => Ty::String,
        Type::Float64 => Ty::Float64,
        Type::None => Ty::None,
        Type::Named(name, args) if name == "List" && args.len() == 1 => {
            let elem = match &args[0] {
                crate::ast::ParamArg::Type(ty) => unchecked_ty(ty),
                _ => Ty::None,
            };
            Ty::List(Box::new(elem))
        }
        Type::Named(name, args) => Ty::Struct(
            name.clone(),
            args.iter()
                .map(|arg| match arg {
                    crate::ast::ParamArg::Type(ty) => crate::types::TyArg::Ty(unchecked_ty(ty)),
                    crate::ast::ParamArg::Value(expr) => {
                        crate::types::TyArg::Val(match &expr.kind {
                            ExprKind::Int(value) => crate::CtValue::Int(*value),
                            _ => crate::CtValue::Int(0),
                        })
                    }
                })
                .collect(),
        ),
        Type::SelfType | Type::SelfParam(_) | Type::Assoc { .. } => Ty::None,
        Type::Func { .. } => Ty::None,
        Type::Ref(inner) => unchecked_ty(inner),
    }
}

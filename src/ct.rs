//! The shared compile-time value model.
//!
//! `CtValue` is the one representation of a compile-time value across the
//! compiler: the [`comptime`](crate::comptime) elaboration pass builds and folds
//! them (`comptime` constants, `comptime if`/`for`, CTFE), and the
//! [`checker`](crate::checker) uses them for value-parameter arguments
//! (`FixedBuffer[8]`, `SIMD[DType.int32, 4]`). Consolidating the two former
//! representations (comptime's own value enum and the checker's former `CtVal`)
//! here keeps the two phases speaking the same language — a prerequisite for
//! type-valued compile-time members.
//!
//! Scalar values and recursively materializable tuples/lists have a runtime
//! literal form; `Type`, `Reflected`, and `Param` are compile-time-only.

use crate::ast::{Expr, ExprKind};
use crate::literal::{FloatLiteral, IntLiteral};
use crate::token::Span;
use crate::types::{Ty, list_element, tuple_elements};
use std::fmt;

/// A compile-time value. Scalar values drive folding; `Tuple`/`List`
/// let `comptime for` iterate compile-time collections; `Type` carries a
/// semantic type for associated comptime members; `Param` is a symbolic value
/// parameter while a generic body is being checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CtValue {
    /// An already-materialized machine `Int` compile-time value. Compiler
    /// generated indices, lengths, and value parameters use this variant.
    Int(i64),
    UInt(u64),
    /// The bits of an already-materialized `Float64` compile-time value.
    Float(u64),
    /// An arbitrary-precision integer literal which has not yet been
    /// materialized into a fixed-width scalar.
    IntLiteral(IntLiteral),
    /// An exact finite floating literal which has not yet been materialized.
    FloatLiteral(FloatLiteral),
    Bool(bool),
    Str(String),
    Tuple(Vec<CtValue>),
    List(Vec<CtValue>),
    /// A `DType.<dt>` compile-time value — the binding of a `[dtype: DType]`
    /// value parameter. Materializes as the member spelling, which type
    /// resolution already accepts inside `SIMD[...]`/`Scalar[...]` brackets.
    Dtype(crate::ast::Dtype),
    /// A frozen struct instance (declaration-ordered fields) — the binding of
    /// a struct-typed value parameter such as `[e: Extent]`. Freezing is
    /// restricted to structs constructible fieldwise from recursively
    /// freezable fields, so materialization is always the fieldwise
    /// construction call.
    Struct {
        name: String,
        fields: Vec<(String, CtValue)>,
    },
    Type(Box<Ty>),
    /// The zero-sized compile-time handle produced by current Mojo's
    /// `reflect[T]` API. Field selection returns another handle, allowing
    /// `.field[name]` / `.field_at[index]` chains to terminate in `.T`.
    Reflected(Box<Ty>),
    Param(String),
}

/// A canonical dependent compile-time expression retained in generic metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CtExpr {
    Value(CtValue),
    Param(String),
    Neg(Box<CtExpr>),
    Add(Box<CtExpr>, Box<CtExpr>),
    Sub(Box<CtExpr>, Box<CtExpr>),
    Mul(Box<CtExpr>, Box<CtExpr>),
    FloorDiv(Box<CtExpr>, Box<CtExpr>),
    Mod(Box<CtExpr>, Box<CtExpr>),
    Pow(Box<CtExpr>, Box<CtExpr>),
}

impl CtExpr {
    /// Collect the symbolic compile-time binders referenced by this expression.
    /// The verifier uses this to reject dependent types whose index escaped its
    /// generic declaration scope.
    pub fn referenced_parameters(&self, output: &mut std::collections::HashSet<String>) {
        use CtExpr::*;
        match self {
            Param(name) => {
                output.insert(name.clone());
            }
            Value(_) => {}
            Neg(value) => value.referenced_parameters(output),
            Add(left, right)
            | Sub(left, right)
            | Mul(left, right)
            | FloorDiv(left, right)
            | Mod(left, right)
            | Pow(left, right) => {
                left.referenced_parameters(output);
                right.referenced_parameters(output);
            }
        }
    }

    /// Alpha-rename symbolic binders while preserving the expression tree.
    pub fn rename_parameters(&self, names: &std::collections::HashMap<String, String>) -> Self {
        use CtExpr::*;
        match self {
            Value(value) => Value(value.clone()),
            Param(name) => Param(names.get(name).cloned().unwrap_or_else(|| name.clone())),
            Neg(value) => Neg(Box::new(value.rename_parameters(names))),
            Add(left, right) => Add(
                Box::new(left.rename_parameters(names)),
                Box::new(right.rename_parameters(names)),
            ),
            Sub(left, right) => Sub(
                Box::new(left.rename_parameters(names)),
                Box::new(right.rename_parameters(names)),
            ),
            Mul(left, right) => Mul(
                Box::new(left.rename_parameters(names)),
                Box::new(right.rename_parameters(names)),
            ),
            FloorDiv(left, right) => FloorDiv(
                Box::new(left.rename_parameters(names)),
                Box::new(right.rename_parameters(names)),
            ),
            Mod(left, right) => Mod(
                Box::new(left.rename_parameters(names)),
                Box::new(right.rename_parameters(names)),
            ),
            Pow(left, right) => Pow(
                Box::new(left.rename_parameters(names)),
                Box::new(right.rename_parameters(names)),
            ),
        }
    }

    pub fn evaluate(
        &self,
        parameters: &std::collections::HashMap<String, CtValue>,
    ) -> Option<CtValue> {
        use CtExpr::*;
        match self {
            Value(value) => Some(value.clone()),
            Param(name) => parameters.get(name).cloned(),
            Neg(value) => match value.evaluate(parameters)? {
                CtValue::Int(value) => value.checked_neg().map(CtValue::Int),
                CtValue::IntLiteral(value) => Some(CtValue::IntLiteral(value.neg())),
                _ => None,
            },
            Add(left, right) => match (left.evaluate(parameters)?, right.evaluate(parameters)?) {
                (CtValue::Int(left), CtValue::Int(right)) => {
                    left.checked_add(right).map(CtValue::Int)
                }
                (CtValue::IntLiteral(left), CtValue::IntLiteral(right)) => {
                    Some(CtValue::IntLiteral(left.add(&right)))
                }
                (CtValue::Int(left), CtValue::IntLiteral(right)) => {
                    Some(CtValue::IntLiteral(IntLiteral::from(left).add(&right)))
                }
                (CtValue::IntLiteral(left), CtValue::Int(right)) => {
                    Some(CtValue::IntLiteral(left.add(&IntLiteral::from(right))))
                }
                (CtValue::Str(left), CtValue::Str(right)) => Some(CtValue::Str(left + &right)),
                _ => None,
            },
            Sub(left, right) => int_binary(
                left,
                right,
                parameters,
                |a, b| a.checked_sub(b),
                |a, b| Some(a.sub(b)),
            ),
            Mul(left, right) => int_binary(
                left,
                right,
                parameters,
                |a, b| a.checked_mul(b),
                |a, b| Some(a.mul(b)),
            ),
            FloorDiv(left, right) => int_binary(
                left,
                right,
                parameters,
                |a, b| a.checked_div_euclid(b),
                IntLiteral::floor_div,
            ),
            Mod(left, right) => int_binary(
                left,
                right,
                parameters,
                |a, b| a.checked_rem_euclid(b),
                IntLiteral::floor_mod,
            ),
            Pow(left, right) => int_binary(
                left,
                right,
                parameters,
                |a, b| u32::try_from(b).ok().and_then(|b| a.checked_pow(b)),
                IntLiteral::pow,
            ),
        }
    }
}

fn int_binary(
    left: &CtExpr,
    right: &CtExpr,
    parameters: &std::collections::HashMap<String, CtValue>,
    operation: impl FnOnce(i64, i64) -> Option<i64>,
    literal_operation: impl FnOnce(&IntLiteral, &IntLiteral) -> Option<IntLiteral>,
) -> Option<CtValue> {
    match (left.evaluate(parameters)?, right.evaluate(parameters)?) {
        (CtValue::Int(left), CtValue::Int(right)) => operation(left, right).map(CtValue::Int),
        (CtValue::IntLiteral(left), CtValue::IntLiteral(right)) => {
            literal_operation(&left, &right).map(CtValue::IntLiteral)
        }
        (CtValue::Int(left), CtValue::IntLiteral(right)) => {
            literal_operation(&left.into(), &right).map(CtValue::IntLiteral)
        }
        (CtValue::IntLiteral(left), CtValue::Int(right)) => {
            literal_operation(&left, &right.into()).map(CtValue::IntLiteral)
        }
        _ => None,
    }
}

impl CtValue {
    /// Materialize an exact literal into the compile-time representation of a
    /// declared scalar type. Values which are already materialized are kept as
    /// is. This is the checked boundary used by value parameters and defaults;
    /// it deliberately leaves uncontextualized literal values exact.
    pub fn materialize_as(self, ty: &Ty) -> Option<Self> {
        match (self, ty) {
            (value @ CtValue::Int(_), Ty::Int)
            | (value @ CtValue::UInt(_), Ty::UInt)
            | (value @ CtValue::Float(_), Ty::Float64)
            | (value @ CtValue::IntLiteral(_), Ty::IntLiteral)
            | (value @ CtValue::FloatLiteral(_), Ty::FloatLiteral)
            | (value @ CtValue::Bool(_), Ty::Bool)
            | (value @ CtValue::Str(_), Ty::StringLiteral) => Some(value),
            (value @ CtValue::Dtype(_), Ty::Dtype) => Some(value),
            (value @ CtValue::Struct { .. }, Ty::Struct(target, _)) => {
                let CtValue::Struct { name, .. } = &value else {
                    unreachable!("guard established a struct value");
                };
                (name == target).then_some(value)
            }
            (CtValue::IntLiteral(value), Ty::Int) => value.wrapping_signed(64).map(CtValue::Int),
            (CtValue::IntLiteral(value), Ty::UInt) => {
                value.wrapping_unsigned(64).map(CtValue::UInt)
            }
            (CtValue::IntLiteral(value), Ty::Float64) => {
                value.to_f64().map(|value| CtValue::Float(value.to_bits()))
            }
            (CtValue::FloatLiteral(value), Ty::Float64) => {
                value.to_f64().map(|value| CtValue::Float(value.to_bits()))
            }
            (CtValue::Tuple(values), Ty::Tuple(types)) if values.len() == types.len() => values
                .into_iter()
                .zip(types)
                .map(|(value, ty)| value.materialize_as(ty))
                .collect::<Option<Vec<_>>>()
                .map(CtValue::Tuple),
            (CtValue::List(values), Ty::ComptimeList(element)) => values
                .into_iter()
                .map(|value| value.materialize_as(element))
                .collect::<Option<Vec<_>>>()
                .map(CtValue::List),
            (CtValue::List(values), target) if list_element(target).is_some() => {
                let element = list_element(target).expect("guard established List element");
                values
                    .into_iter()
                    .map(|value| value.materialize_as(element))
                    .collect::<Option<Vec<_>>>()
                    .map(CtValue::List)
            }
            (CtValue::Tuple(values), target)
                if tuple_elements(target).is_some_and(|types| types.len() == values.len()) =>
            {
                values
                    .into_iter()
                    .zip(tuple_elements(target).expect("guard established Tuple elements"))
                    .map(|(value, ty)| value.materialize_as(ty))
                    .collect::<Option<Vec<_>>>()
                    .map(CtValue::Tuple)
            }
            _ => None,
        }
    }

    /// Materialize this value as a literal expression, or `None` when it has no
    /// runtime form (a symbolic `Param`, or a collection containing one).
    pub fn materialize(&self, span: Span) -> Option<Expr> {
        let kind = match self {
            CtValue::Int(n) => ExprKind::Int(IntLiteral::from(*n)),
            CtValue::UInt(n) => ExprKind::Int(IntLiteral::from(*n)),
            CtValue::Float(bits) => ExprKind::Float(FloatLiteral::from_f64(f64::from_bits(*bits))?),
            CtValue::IntLiteral(value) => ExprKind::Int(value.clone()),
            CtValue::FloatLiteral(value) => ExprKind::Float(value.clone()),
            CtValue::Bool(b) => ExprKind::Bool(*b),
            CtValue::Str(s) => ExprKind::Str(s.clone()),
            CtValue::Tuple(vs) => ExprKind::TupleLit(materialize_all(vs, span)?),
            CtValue::List(vs) => ExprKind::ListLit(materialize_all(vs, span)?),
            CtValue::Dtype(dtype) => ExprKind::Member {
                object: Box::new(Expr {
                    kind: ExprKind::Identifier("DType".to_string()),
                    span,
                    source: None,
                    syntax_id: crate::token::SyntaxId::fresh(),
                }),
                field: dtype.name().to_string(),
            },
            // The fieldwise construction call; freezing guaranteed a matching
            // constructor exists.
            CtValue::Struct { name, fields } => ExprKind::Call {
                name: name.clone(),
                param_args: Vec::new(),
                args: fields
                    .iter()
                    .map(|(_, value)| value.materialize(span))
                    .collect::<Option<Vec<_>>>()?,
                kwargs: Vec::new(),
            },
            CtValue::Type(_) | CtValue::Reflected(_) => return None,
            CtValue::Param(_) => return None,
        };
        Some(Expr {
            kind,
            span,
            source: None,
            syntax_id: crate::token::SyntaxId::fresh(),
        })
    }
}

fn materialize_all(vs: &[CtValue], span: Span) -> Option<Vec<Expr>> {
    vs.iter().map(|v| v.materialize(span)).collect()
}

impl fmt::Display for CtValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CtValue::Int(n) => write!(f, "{n}"),
            CtValue::UInt(n) => write!(f, "{n}u"),
            CtValue::Float(bits) => write!(f, "{:?}", f64::from_bits(*bits)),
            CtValue::IntLiteral(value) => write!(f, "{value}"),
            CtValue::FloatLiteral(value) => write!(f, "{value}"),
            CtValue::Bool(b) => write!(f, "{b}"),
            CtValue::Str(s) => write!(f, "{s:?}"),
            CtValue::Dtype(dtype) => write!(f, "DType.{}", dtype.name()),
            CtValue::Struct { name, fields } => {
                write!(f, "{name}(")?;
                for (index, (_, value)) in fields.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{value}")?;
                }
                write!(f, ")")
            }
            CtValue::Type(ty) => write!(f, "{ty}"),
            CtValue::Reflected(ty) => write!(f, "reflect[{ty}]"),
            CtValue::Param(name) => write!(f, "{name}"),
            CtValue::Tuple(vs) | CtValue::List(vs) => {
                let (open, close) = match self {
                    CtValue::Tuple(_) => ('(', ')'),
                    _ => ('[', ']'),
                };
                write!(f, "{open}")?;
                for (i, v) in vs.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{v}")?;
                }
                write!(f, "{close}")
            }
        }
    }
}

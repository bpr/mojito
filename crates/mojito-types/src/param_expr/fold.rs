//! The one concrete folder for compile-time scalar operators.
//!
//! Every phase that needs the value of `left op right` over two concrete
//! [`CtValue`]s calls [`fold_infix`]: the checker's constant evaluators, the
//! elaborator, native monomorphization, and the canonicalizing
//! [`ParamContext`](super::ParamContext) constructors. Name resolution, lazy
//! branch selection, and CTFE stay with their owners; only operator semantics
//! live here.
//!
//! Arithmetic domains are explicit. Exact literals never narrow. Machine
//! `Int`/`UInt` addition, subtraction, and multiplication wrap at 64 bits, and
//! `//`/`%` floor toward the divisor's sign, as the pinned Mojo folds them
//! (`assets/ok/param_expr_arithmetic.mojo`,
//! `param_expr_overflow.mojo`). A literal beside a machine operand converts
//! to the machine type first.

use super::ParamError;
use crate::ct::{CtLane, CtValue, wrap_lane};
use mojito_ast::ast::InfixOp;
use mojito_common::literal::{FloatLiteral, IntLiteral};

/// Fold `left op right` over two concrete values.
///
/// `and`/`or`, membership, and any operator with a lazily evaluated operand
/// belong to the caller; this entry receives operands that are already
/// required.
pub fn fold_infix(op: InfixOp, left: &CtValue, right: &CtValue) -> Result<CtValue, ParamError> {
    use InfixOp::{Eq, Ge, Gt, Le, Lt, Ne};
    match (left, right) {
        (CtValue::Type(a), CtValue::Type(b)) => {
            return match op {
                Eq => Ok(CtValue::Bool(a == b)),
                Ne => Ok(CtValue::Bool(a != b)),
                _ => Err(unsupported(
                    "only == and != are defined for compile-time types",
                )),
            };
        }
        (CtValue::Str(a), CtValue::Str(b)) => {
            return match op {
                InfixOp::Add => Ok(CtValue::Str(format!("{a}{b}"))),
                Eq => Ok(CtValue::Bool(a == b)),
                Ne => Ok(CtValue::Bool(a != b)),
                _ => Err(unsupported("unsupported compile-time String operator")),
            };
        }
        (CtValue::Bool(a), CtValue::Bool(b)) => {
            return match op {
                Eq => Ok(CtValue::Bool(a == b)),
                Ne | InfixOp::BitXor => Ok(CtValue::Bool(a != b)),
                InfixOp::And | InfixOp::BitAnd => Ok(CtValue::Bool(*a && *b)),
                InfixOp::Or | InfixOp::BitOr => Ok(CtValue::Bool(*a || *b)),
                _ => Err(unsupported("unsupported compile-time Bool operator")),
            };
        }
        _ => {}
    }
    if matches!(op, Eq | Ne | Lt | Gt | Le | Ge) {
        return compare(op, left, right).map(CtValue::Bool);
    }
    match (left, right) {
        (
            CtValue::Simd { dtype, lanes: a },
            CtValue::Simd {
                dtype: other,
                lanes: b,
            },
        ) if dtype == other && a.len() == b.len() => a
            .iter()
            .zip(b)
            .map(|(x, y)| match (x, y) {
                (CtLane::Int(x), CtLane::Int(y)) => {
                    let value = match op {
                        InfixOp::BitXor => x ^ y,
                        InfixOp::BitAnd => x & y,
                        InfixOp::BitOr => x | y,
                        InfixOp::Add => x.wrapping_add(*y),
                        InfixOp::Sub => x.wrapping_sub(*y),
                        InfixOp::Mul => x.wrapping_mul(*y),
                        _ => return None,
                    };
                    Some(CtLane::Int(wrap_lane(*dtype, value)))
                }
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .map(|lanes| CtValue::Simd {
                dtype: *dtype,
                lanes,
            })
            .ok_or_else(|| unsupported("unsupported compile-time SIMD operator")),
        (CtValue::Int(a), CtValue::Int(b)) => fold_int(op, *a, *b).map(CtValue::Int),
        (CtValue::UInt(a), CtValue::UInt(b)) => fold_uint(op, *a, *b).map(CtValue::UInt),
        (CtValue::IntLiteral(a), CtValue::IntLiteral(b)) => fold_int_literal(op, a, b),
        (CtValue::FloatLiteral(a), CtValue::FloatLiteral(b)) => {
            fold_float_literal(op, a, b).map(CtValue::FloatLiteral)
        }
        (CtValue::Float(a), CtValue::Float(b)) => {
            fold_float(op, f64::from_bits(*a), f64::from_bits(*b))
        }
        // True division of integers is exact, whatever the operands' width.
        (CtValue::Int(a), CtValue::IntLiteral(b)) if op == InfixOp::Div => {
            fold_int_literal(op, &IntLiteral::from(*a), b)
        }
        (CtValue::IntLiteral(a), CtValue::Int(b)) if op == InfixOp::Div => {
            fold_int_literal(op, a, &IntLiteral::from(*b))
        }
        // A literal beside a machine operand takes the machine type.
        (CtValue::Int(a), CtValue::IntLiteral(b)) => {
            fold_int(op, *a, machine_int(b)?).map(CtValue::Int)
        }
        (CtValue::IntLiteral(a), CtValue::Int(b)) => {
            fold_int(op, machine_int(a)?, *b).map(CtValue::Int)
        }
        (CtValue::UInt(a), CtValue::IntLiteral(b)) => {
            fold_uint(op, *a, machine_uint(b)?).map(CtValue::UInt)
        }
        (CtValue::IntLiteral(a), CtValue::UInt(b)) => {
            fold_uint(op, machine_uint(a)?, *b).map(CtValue::UInt)
        }
        (CtValue::IntLiteral(a), CtValue::FloatLiteral(b)) => {
            fold_float_literal(op, &FloatLiteral::from_int(a), b).map(CtValue::FloatLiteral)
        }
        (CtValue::FloatLiteral(a), CtValue::IntLiteral(b)) => {
            fold_float_literal(op, a, &FloatLiteral::from_int(b)).map(CtValue::FloatLiteral)
        }
        (CtValue::Float(a), literal @ (CtValue::IntLiteral(_) | CtValue::FloatLiteral(_))) => {
            fold_float(op, f64::from_bits(*a), machine_float(literal)?)
        }
        (literal @ (CtValue::IntLiteral(_) | CtValue::FloatLiteral(_)), CtValue::Float(b)) => {
            fold_float(op, machine_float(literal)?, f64::from_bits(*b))
        }
        _ => Err(unsupported("unsupported compile-time operands")),
    }
}

/// Fold unary `-` over one concrete value.
pub fn fold_neg(value: &CtValue) -> Result<CtValue, ParamError> {
    match value {
        CtValue::Int(value) => Ok(CtValue::Int(value.wrapping_neg())),
        CtValue::IntLiteral(value) => Ok(CtValue::IntLiteral(value.neg())),
        CtValue::FloatLiteral(value) => Ok(CtValue::FloatLiteral(value.neg())),
        CtValue::Float(bits) => Ok(CtValue::Float((-f64::from_bits(*bits)).to_bits())),
        _ => Err(unsupported("unary '-' expects a comptime numeric value")),
    }
}

/// Compare two concrete values numerically (`DType` supports `==`/`!=`).
///
/// Numbers compare by exact rational value across kinds, so a literal and a
/// machine operand compare mathematically. A NaN operand has no exact value
/// and is an error rather than an ordering.
pub fn compare(op: InfixOp, left: &CtValue, right: &CtValue) -> Result<bool, ParamError> {
    use InfixOp::{Eq, Ge, Gt, Le, Lt, Ne};
    if let (CtValue::Dtype(left), CtValue::Dtype(right)) = (left, right) {
        return match op {
            Eq => Ok(left == right),
            Ne => Ok(left != right),
            _ => Err(unsupported(
                "'DType' supports only '==' and '!=' comparisons",
            )),
        };
    }
    let (Some(left), Some(right)) = (exact(left), exact(right)) else {
        return Err(unsupported("numeric comparison expects numeric operands"));
    };
    let ordering = left.as_rational().cmp(right.as_rational());
    Ok(match op {
        Eq => ordering.is_eq(),
        Ne => !ordering.is_eq(),
        Lt => ordering.is_lt(),
        Gt => ordering.is_gt(),
        Le => !ordering.is_gt(),
        Ge => !ordering.is_lt(),
        _ => return Err(unsupported("unsupported compile-time comparison")),
    })
}

/// The integer a concrete value denotes, across the three integer kinds.
pub fn integer_value(value: &CtValue) -> Option<IntLiteral> {
    match value {
        CtValue::Int(value) => Some((*value).into()),
        CtValue::UInt(value) => Some((*value).into()),
        CtValue::IntLiteral(value) => Some(value.clone()),
        _ => None,
    }
}

fn exact(value: &CtValue) -> Option<FloatLiteral> {
    match value {
        CtValue::Float(bits) => FloatLiteral::from_f64(f64::from_bits(*bits)),
        CtValue::FloatLiteral(value) => Some(value.clone()),
        integer => integer_value(integer).map(|value| FloatLiteral::from_int(&value)),
    }
}

fn fold_int(op: InfixOp, a: i64, b: i64) -> Result<i64, ParamError> {
    Ok(match op {
        InfixOp::Add => a.wrapping_add(b),
        InfixOp::Sub => a.wrapping_sub(b),
        InfixOp::Mul => a.wrapping_mul(b),
        InfixOp::FloorDiv | InfixOp::Mod if b == 0 => return Err(arithmetic("division by zero")),
        InfixOp::FloorDiv => {
            let quotient = a.wrapping_div(b);
            if a.wrapping_rem(b) != 0 && ((a < 0) != (b < 0)) {
                quotient.wrapping_sub(1)
            } else {
                quotient
            }
        }
        InfixOp::Mod => {
            let remainder = a.wrapping_rem(b);
            if remainder != 0 && ((remainder < 0) != (b < 0)) {
                remainder.wrapping_add(b)
            } else {
                remainder
            }
        }
        InfixOp::Pow if b < 0 => return Err(arithmetic("negative exponent")),
        InfixOp::Pow => u32::try_from(b)
            .ok()
            .and_then(|exponent| a.checked_pow(exponent))
            .ok_or_else(|| arithmetic("compile-time integer overflow"))?,
        InfixOp::Shl => a.wrapping_shl(shift_amount(b)?),
        InfixOp::Shr => a.wrapping_shr(shift_amount(b)?),
        InfixOp::BitAnd => a & b,
        InfixOp::BitOr => a | b,
        InfixOp::BitXor => a ^ b,
        _ => return Err(unsupported("unsupported compile-time operator")),
    })
}

fn fold_uint(op: InfixOp, a: u64, b: u64) -> Result<u64, ParamError> {
    Ok(match op {
        InfixOp::Add => a.wrapping_add(b),
        InfixOp::Sub => a.wrapping_sub(b),
        InfixOp::Mul => a.wrapping_mul(b),
        InfixOp::FloorDiv | InfixOp::Mod if b == 0 => return Err(arithmetic("division by zero")),
        InfixOp::FloorDiv => a / b,
        InfixOp::Mod => a % b,
        InfixOp::Pow => u32::try_from(b)
            .ok()
            .and_then(|exponent| a.checked_pow(exponent))
            .ok_or_else(|| arithmetic("compile-time integer overflow"))?,
        InfixOp::Shl => a.wrapping_shl(shift_amount(i64::try_from(b).unwrap_or(-1))?),
        InfixOp::Shr => a.wrapping_shr(shift_amount(i64::try_from(b).unwrap_or(-1))?),
        InfixOp::BitAnd => a & b,
        InfixOp::BitOr => a | b,
        InfixOp::BitXor => a ^ b,
        _ => return Err(unsupported("unsupported compile-time operator")),
    })
}

fn shift_amount(amount: i64) -> Result<u32, ParamError> {
    u32::try_from(amount)
        .ok()
        .filter(|amount| *amount < 64)
        .ok_or_else(|| arithmetic("invalid or resource-limited compile-time shift"))
}

fn fold_int_literal(op: InfixOp, a: &IntLiteral, b: &IntLiteral) -> Result<CtValue, ParamError> {
    let value = match op {
        InfixOp::Add => Some(CtValue::IntLiteral(a.add(b))),
        InfixOp::Sub => Some(CtValue::IntLiteral(a.sub(b))),
        InfixOp::Mul => Some(CtValue::IntLiteral(a.mul(b))),
        InfixOp::Div => FloatLiteral::from_int(a)
            .div(&FloatLiteral::from_int(b))
            .map(CtValue::FloatLiteral),
        InfixOp::FloorDiv => a.floor_div(b).map(CtValue::IntLiteral),
        InfixOp::Mod => a.floor_mod(b).map(CtValue::IntLiteral),
        InfixOp::Pow => a.pow(b).map(CtValue::IntLiteral),
        InfixOp::Shl => a.shl(b).map(CtValue::IntLiteral),
        InfixOp::Shr => a.shr(b).map(CtValue::IntLiteral),
        InfixOp::BitAnd => Some(CtValue::IntLiteral(a.bitand(b))),
        InfixOp::BitOr => Some(CtValue::IntLiteral(a.bitor(b))),
        InfixOp::BitXor => Some(CtValue::IntLiteral(a.bitxor(b))),
        _ => return Err(unsupported("unsupported exact compile-time operator")),
    };
    value.ok_or_else(|| arithmetic("invalid exact compile-time arithmetic"))
}

fn fold_float_literal(
    op: InfixOp,
    a: &FloatLiteral,
    b: &FloatLiteral,
) -> Result<FloatLiteral, ParamError> {
    let value = match op {
        InfixOp::Add => Some(a.add(b)),
        InfixOp::Sub => Some(a.sub(b)),
        InfixOp::Mul => Some(a.mul(b)),
        InfixOp::Div => a.div(b),
        InfixOp::FloorDiv => a.floor_div(b),
        InfixOp::Mod => a.floor_mod(b),
        InfixOp::Pow => b
            .to_int_if_whole()
            .and_then(|exponent| a.pow_int(&exponent)),
        _ => {
            return Err(unsupported("unsupported exact compile-time float operator"));
        }
    };
    value.ok_or_else(|| arithmetic("invalid exact compile-time arithmetic"))
}

/// Machine `Float64` arithmetic is IEEE and is never reassociated; only the
/// four field operations fold.
fn fold_float(op: InfixOp, a: f64, b: f64) -> Result<CtValue, ParamError> {
    let value = match op {
        InfixOp::Add => a + b,
        InfixOp::Sub => a - b,
        InfixOp::Mul => a * b,
        InfixOp::Div => a / b,
        _ => return Err(unsupported("unsupported compile-time operands")),
    };
    Ok(CtValue::Float(value.to_bits()))
}

fn machine_int(value: &IntLiteral) -> Result<i64, ParamError> {
    value
        .wrapping_signed(64)
        .ok_or_else(|| arithmetic("compile-time integer overflow"))
}

fn machine_uint(value: &IntLiteral) -> Result<u64, ParamError> {
    value
        .wrapping_unsigned(64)
        .ok_or_else(|| arithmetic("compile-time integer overflow"))
}

fn machine_float(value: &CtValue) -> Result<f64, ParamError> {
    match value {
        CtValue::IntLiteral(value) => value.to_f64(),
        CtValue::FloatLiteral(value) => value.to_f64(),
        _ => None,
    }
    .ok_or_else(|| arithmetic("invalid exact compile-time arithmetic"))
}

fn arithmetic(message: &str) -> ParamError {
    ParamError::Arithmetic(message.to_string())
}

fn unsupported(message: &str) -> ParamError {
    ParamError::Unsupported(message.to_string())
}

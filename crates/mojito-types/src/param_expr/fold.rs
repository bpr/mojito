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
//!
//! SIMD lanes follow the VM's own lane rule ([`int_lane_binop`],
//! [`float_lane_binop`], [`lane_compare`], [`bool_lane_binop`]): a literal or
//! a width-1 operand beside a lane splats through
//! [`CtLane::of_int_literal`]/[`CtLane::of_float_literal`], and a comparison
//! is `pop.cmp`, which gives a `bool` mask under the ordered predicates
//! (a NaN lane is unequal to nothing).

use super::ParamError;
use crate::ct::{
    CtLane, CtValue, bool_lane_binop, float_lane_binop, int_lane_binop, lane_compare, wrap_lane,
};
use crate::types::same_type;
use mojito_ast::ast::{Dtype, InfixOp};
use mojito_common::literal::{FloatLiteral, IntLiteral};

/// Fold `left op right` over two concrete values.
///
/// `and`/`or`, membership, and any operator with a lazily evaluated operand
/// belong to the caller; this entry receives operands that are already
/// required.
pub fn fold_infix(op: InfixOp, left: &CtValue, right: &CtValue) -> Result<CtValue, ParamError> {
    use InfixOp::{Eq, Ge, Gt, Le, Lt, Ne};
    match (left, right) {
        // A KGEN string or an untyped attribute implements no operator.
        (CtValue::KgenString(_), _) | (_, CtValue::KgenString(_)) => {
            return Err(unsupported(&format!(
                "'{}' does not implement the '{}' method",
                crate::types::KGEN_STRING_TYPE_SPELLING,
                op.dunder().unwrap_or("operator")
            )));
        }
        (CtValue::MlirAttr(_), _) | (_, CtValue::MlirAttr(_)) => {
            return Err(unsupported(&format!(
                "'{}' does not implement the '{}' method",
                crate::types::KGEN_DEFERRED_TYPE_SPELLING,
                op.dunder().unwrap_or("operator")
            )));
        }
        (CtValue::Type(a), CtValue::Type(b)) => {
            return match op {
                Eq => Ok(CtValue::Bool(same_type(a, b))),
                Ne => Ok(CtValue::Bool(!same_type(a, b))),
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
        (CtValue::Simd { .. }, _) | (_, CtValue::Simd { .. }) => {
            return fold_lanes(op, left, right);
        }
        _ => {}
    }
    if matches!(op, Eq | Ne | Lt | Gt | Le | Ge) {
        return compare(op, left, right).map(CtValue::Bool);
    }
    match (left, right) {
        (CtValue::Int(a), CtValue::Int(b)) => fold_int(op, *a, *b).map(CtValue::Int),
        (CtValue::UInt(a), CtValue::UInt(b)) => fold_uint(op, *a, *b).map(CtValue::UInt),
        (CtValue::Index(a), CtValue::Index(b)) => fold_int(op, *a, *b).map(CtValue::Index),
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
        (CtValue::Index(a), CtValue::IntLiteral(b)) => {
            fold_int(op, *a, machine_int(b)?).map(CtValue::Index)
        }
        (CtValue::IntLiteral(a), CtValue::Index(b)) => {
            fold_int(op, machine_int(a)?, *b).map(CtValue::Index)
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
        CtValue::Simd { dtype, lanes } => map_lanes(*dtype, lanes, |lane| match lane {
            CtLane::Int(x) => Some(CtLane::Int(wrap_lane(*dtype, x.wrapping_neg()))),
            CtLane::Float(bits) => Some(CtLane::Float((-f64::from_bits(*bits)).to_bits())),
            CtLane::Bool(_) => None,
        })
        .ok_or_else(|| unsupported("unary '-' expects numeric SIMD lanes")),
        _ => Err(unsupported("unary '-' expects a comptime numeric value")),
    }
}

/// Fold unary `~` over one concrete value: an integer's two's-complement
/// inversion, which an exact literal takes as `-value - 1`.
pub fn fold_invert(value: &CtValue) -> Result<CtValue, ParamError> {
    match value {
        CtValue::Int(value) => Ok(CtValue::Int(!value)),
        CtValue::UInt(value) => Ok(CtValue::UInt(!value)),
        CtValue::Index(value) => Ok(CtValue::Index(!value)),
        CtValue::IntLiteral(value) => Ok(CtValue::IntLiteral(
            value.neg().sub(&IntLiteral::from(1i64)),
        )),
        CtValue::Bool(value) => Ok(CtValue::Bool(!value)),
        CtValue::Simd { dtype, lanes } => map_lanes(*dtype, lanes, |lane| match lane {
            CtLane::Int(x) => Some(CtLane::Int(wrap_lane(*dtype, !x))),
            CtLane::Bool(x) => Some(CtLane::Bool(!x)),
            CtLane::Float(_) => None,
        })
        .ok_or_else(|| unsupported("unary '~' expects integer or bool SIMD lanes")),
        _ => Err(unsupported("unary '~' expects a comptime integer value")),
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
    let (Some(exact_left), Some(exact_right)) = (exact(left), exact(right)) else {
        // A NaN or an infinity has no exact value: it compares as
        // `pop.cmp`'s ordered predicates do.
        let float = |value: &CtValue| match value {
            CtValue::Float(bits) => Some(f64::from_bits(*bits)),
            _ => machine_float(value).ok(),
        };
        return float(left)
            .zip(float(right))
            .and_then(|(left, right)| lane_compare(op, left, right))
            .ok_or_else(|| unsupported("numeric comparison expects numeric operands"));
    };
    let ordering = exact_left.as_rational().cmp(exact_right.as_rational());
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

/// Fold the source comparison `left op right`, upstream's folded
/// `SIMD.__eq__`/`__ne__`/`__lt__` family, to a `Bool`.
///
/// [`fold_infix`] is `pop.cmp`, elementwise over SIMD lanes; the dunder
/// reads its mask as a `Bool`: `==` when every lane holds, and an ordered
/// operator when any lane holds, as a folded call drops its `comptime
/// assert` that the operands be scalars. `!=` is `not ==` over every
/// operand kind, so a NaN makes it hold where `pop.cmp ne` does not. Any
/// other operator, and a non-SIMD comparison, folds as [`fold_infix`]
/// folds it.
pub fn fold_comparison(
    op: InfixOp,
    left: &CtValue,
    right: &CtValue,
) -> Result<CtValue, ParamError> {
    use InfixOp::{Eq, Ge, Gt, Le, Lt, Ne};
    if op == Ne
        && let CtValue::Bool(equal) = fold_comparison(Eq, left, right)?
    {
        return Ok(CtValue::Bool(!equal));
    }
    let simd = matches!(left, CtValue::Simd { .. }) || matches!(right, CtValue::Simd { .. });
    if !simd || !matches!(op, Eq | Lt | Gt | Le | Ge) {
        return fold_infix(op, left, right);
    }
    let CtValue::Simd { lanes, .. } = fold_infix(op, left, right)? else {
        return Err(unsupported("a SIMD comparison expects a mask"));
    };
    let mut mask = lanes.iter().map(|lane| match lane {
        CtLane::Bool(value) => Ok(*value),
        _ => Err(unsupported("a SIMD comparison expects a bool mask")),
    });
    Ok(CtValue::Bool(match op {
        Eq => mask.try_fold(true, |all, lane| lane.map(|lane| all && lane))?,
        _ => mask.try_fold(false, |any, lane| lane.map(|lane| any || lane))?,
    }))
}

/// Whether the source comparison `left op right` holds, as
/// [`fold_comparison`] folds it; `None` when the folder refuses the
/// operands.
pub fn comparison_holds(op: InfixOp, left: &CtValue, right: &CtValue) -> Option<bool> {
    match fold_comparison(op, left, right).ok()? {
        CtValue::Bool(value) => Some(value),
        _ => None,
    }
}

/// `pop.cast_to_builtin` over a concrete value: an `Int`, or a literal
/// materialized to one first, as the `index` it stores.
pub fn fold_cast_to_builtin(value: &CtValue) -> Result<CtValue, ParamError> {
    match value {
        CtValue::Int(value) => Ok(CtValue::Index(*value)),
        CtValue::IntLiteral(value) => machine_int(value).map(CtValue::Index),
        _ => Err(unsupported("'__mlir_index__' expects a compile-time 'Int'")),
    }
}

/// `pop.cast_from_builtin` over a concrete value: an `index` as the `Int`
/// whose storage it is.
pub fn fold_cast_from_builtin(value: &CtValue) -> Result<CtValue, ParamError> {
    match value {
        CtValue::Index(value) => Ok(CtValue::Int(*value)),
        _ => Err(unsupported(&format!(
            "'Int(mlir_value=)' expects a compile-time '{}'",
            crate::types::MLIR_INDEX_TYPE_SPELLING
        ))),
    }
}

/// The integer a concrete value denotes, across the integer kinds.
pub fn integer_value(value: &CtValue) -> Option<IntLiteral> {
    match value {
        CtValue::Int(value) | CtValue::Index(value) => Some((*value).into()),
        CtValue::UInt(value) => Some((*value).into()),
        CtValue::IntLiteral(value) => Some(value.clone()),
        _ => None,
    }
}

/// `value` as `width` lanes of `dtype`, or `None` for a value that is none
/// of these.
///
/// A SIMD value of that shape is its lanes; a width-1 SIMD value, a literal,
/// or a `Bool` into `bool` lanes splats, as the VM splats them (upstream's
/// splat `SIMD.__init__`).
pub fn splat_lanes(value: &CtValue, dtype: Dtype, width: usize) -> Option<Vec<CtLane>> {
    let lane = match value {
        CtValue::Simd { dtype: own, lanes } if *own == dtype && lanes.len() == width => {
            return Some(lanes.clone());
        }
        CtValue::Simd { dtype: own, lanes } if *own == dtype && lanes.len() == 1 => {
            lanes[0].clone()
        }
        CtValue::IntLiteral(literal) => CtLane::of_int_literal(literal, dtype)?,
        CtValue::FloatLiteral(literal) => CtLane::of_float_literal(literal, dtype)?,
        CtValue::Bool(value) if dtype == Dtype::Bool => CtLane::Bool(*value),
        _ => return None,
    };
    Some(vec![lane; width])
}

/// Fold `left op right` lane by lane, as the VM's `simd_binop` computes it:
/// a width-1 SIMD operand or a literal splats to the other operand's shape,
/// and a comparison gives a `bool` mask. A shift whose amount is outside the
/// lane width is refused, since the pinned Mojo rejects it as a constant.
fn fold_lanes(op: InfixOp, left: &CtValue, right: &CtValue) -> Result<CtValue, ParamError> {
    use InfixOp::{Eq, Ge, Gt, Le, Lt, Ne, Shl, Shr};
    let shape = |value: &CtValue| match value {
        CtValue::Simd { dtype, lanes } => Some((*dtype, lanes.len())),
        _ => None,
    };
    let (dtype, width) = match (shape(left), shape(right)) {
        (Some((dtype, a)), Some((_, b))) => (dtype, a.max(b)),
        (Some(shape), None) | (None, Some(shape)) => shape,
        (None, None) => return Err(unsupported("SIMD folding expects a SIMD operand")),
    };
    let is_compare = matches!(op, Eq | Ne | Lt | Gt | Le | Ge);
    let in_width = |amount: &i128| (0..i128::from(dtype.bit_width())).contains(amount);
    splat_lanes(left, dtype, width)
        .zip(splat_lanes(right, dtype, width))
        .and_then(|(xs, ys)| {
            xs.iter()
                .zip(&ys)
                .map(|pair| match pair {
                    (CtLane::Int(a), CtLane::Int(b)) if is_compare => {
                        lane_compare(op, a, b).map(CtLane::Bool)
                    }
                    (CtLane::Int(_), CtLane::Int(b)) if matches!(op, Shl | Shr) && !in_width(b) => {
                        None
                    }
                    (CtLane::Int(a), CtLane::Int(b)) => {
                        int_lane_binop(op, dtype, *a, *b).map(CtLane::Int)
                    }
                    (CtLane::Float(a), CtLane::Float(b)) => {
                        let (a, b) = (f64::from_bits(*a), f64::from_bits(*b));
                        if is_compare {
                            lane_compare(op, a, b).map(CtLane::Bool)
                        } else {
                            float_lane_binop(op, dtype, a, b).map(|x| CtLane::Float(x.to_bits()))
                        }
                    }
                    (CtLane::Bool(a), CtLane::Bool(b)) => {
                        bool_lane_binop(op, *a, *b).map(CtLane::Bool)
                    }
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
        })
        .map(|lanes| CtValue::Simd {
            dtype: if is_compare { Dtype::Bool } else { dtype },
            lanes,
        })
        .ok_or_else(|| unsupported("unsupported compile-time SIMD operator"))
}

fn map_lanes(
    dtype: Dtype,
    lanes: &[CtLane],
    lane: impl Fn(&CtLane) -> Option<CtLane>,
) -> Option<CtValue> {
    lanes
        .iter()
        .map(lane)
        .collect::<Option<Vec<_>>>()
        .map(|lanes| CtValue::Simd { dtype, lanes })
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
        // `SIMD.__floordiv__`/`__mod__` mask a zero divisor to a zero lane.
        InfixOp::FloorDiv | InfixOp::Mod if b == 0 => 0,
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
        InfixOp::FloorDiv | InfixOp::Mod if b == 0 => 0,
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

#[cfg(test)]
mod tests {
    use super::{fold_comparison, fold_infix};
    use crate::ct::{CtLane, CtValue};
    use mojito_ast::ast::{Dtype, InfixOp};
    use mojito_common::literal::{FloatLiteral, IntLiteral};

    fn lane(dtype: Dtype, lane: CtLane) -> CtValue {
        CtValue::Simd {
            dtype,
            lanes: vec![lane],
        }
    }

    #[test]
    fn lanes_fold_by_the_vm_rule() {
        let byte = |value| lane(Dtype::UInt8, CtLane::Int(value));
        let ten = CtValue::IntLiteral(IntLiteral::from(10i64));
        assert_eq!(
            fold_infix(InfixOp::Add, &byte(250), &ten).ok(),
            Some(byte(4))
        );
        assert_eq!(
            fold_infix(InfixOp::Add, &ten, &byte(250)).ok(),
            Some(byte(4))
        );
        assert_eq!(
            fold_infix(InfixOp::Lt, &byte(3), &byte(4)).ok(),
            Some(lane(Dtype::Bool, CtLane::Bool(true)))
        );
        assert!(fold_infix(InfixOp::Shl, &byte(1), &byte(9)).is_err());
        let single = |value: f64| lane(Dtype::Float32, CtLane::Float(value.to_bits()));
        let two = CtValue::FloatLiteral(FloatLiteral::from_int(&IntLiteral::from(2i64)));
        assert_eq!(
            fold_infix(InfixOp::Mul, &single(1.5), &two).ok(),
            Some(single(3.0))
        );
        // Float lanes take `//`, as the VM's `pop.floordiv` does.
        assert_eq!(
            fold_infix(InfixOp::FloorDiv, &single(7.5), &two).ok(),
            Some(single(3.0))
        );
        assert!(fold_infix(InfixOp::Add, &byte(1), &two).is_err());
    }

    #[test]
    fn fold_comparison_reads_the_mask_as_the_dunder() {
        let ints = |lanes: [i64; 4]| CtValue::Simd {
            dtype: Dtype::Int32,
            lanes: lanes
                .into_iter()
                .map(|value| CtLane::Int(i128::from(value)))
                .collect(),
        };
        let three = CtValue::IntLiteral(IntLiteral::from(3i64));
        let fold = |op, left: &CtValue, right: &CtValue| fold_comparison(op, left, right).ok();
        assert_eq!(
            fold(InfixOp::Lt, &ints([4, 1, 4, 4]), &three),
            Some(CtValue::Bool(true))
        );
        assert_eq!(
            fold(InfixOp::Lt, &ints([4, 4, 4, 4]), &three),
            Some(CtValue::Bool(false))
        );
        assert_eq!(
            fold(InfixOp::Eq, &ints([1, 2, 3, 4]), &ints([1, 2, 3, 5])),
            Some(CtValue::Bool(false))
        );
        let nan = CtValue::Simd {
            dtype: Dtype::Float32,
            lanes: vec![CtLane::Float(f64::NAN.to_bits()); 2],
        };
        assert_eq!(fold(InfixOp::Ne, &nan, &nan), Some(CtValue::Bool(true)));
        assert_eq!(fold(InfixOp::Eq, &nan, &nan), Some(CtValue::Bool(false)));
        let scalar_nan = CtValue::Float(f64::NAN.to_bits());
        assert_eq!(
            fold(InfixOp::Ne, &scalar_nan, &scalar_nan),
            Some(CtValue::Bool(true))
        );
        let CtValue::Simd { lanes, .. } = fold_infix(InfixOp::Ne, &nan, &nan).unwrap() else {
            panic!("an elementwise `pop.cmp ne` is a mask");
        };
        assert_eq!(lanes, vec![CtLane::Bool(false); 2]);
    }
}

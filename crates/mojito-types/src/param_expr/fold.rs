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

/// KGEN's `data_to_str` over concrete operands: the text of the view
/// `string`, then of each view in the list `rest`, as a `!kgen.string`.
pub fn fold_data_to_str(string: &CtValue, rest: &CtValue) -> Result<CtValue, ParamError> {
    let (CtValue::Tuple(rest) | CtValue::List(rest)) = rest else {
        return Err(unsupported("'data_to_str' expects a list of string views"));
    };
    let mut bytes = view_bytes(string)?;
    for view in rest {
        bytes.extend(view_bytes(view)?);
    }
    String::from_utf8(bytes)
        .map(CtValue::KgenString)
        .map_err(|_| unsupported("'data_to_str' reads bytes that are not UTF-8"))
}

/// `#pop.string_concat` over two concrete `!kgen.string`s.
pub fn fold_string_concat(left: &CtValue, right: &CtValue) -> Result<CtValue, ParamError> {
    match (left, right) {
        (CtValue::KgenString(left), CtValue::KgenString(right)) => {
            Ok(CtValue::KgenString(format!("{left}{right}")))
        }
        _ => Err(unsupported(
            "'#pop.string_concat' expects two '!kgen.string' values",
        )),
    }
}

/// KGEN's `target_get_field` over a concrete target and field name.
pub fn fold_target_get_field(target: &CtValue, name: &CtValue) -> Result<CtValue, ParamError> {
    let (target, name) = target_operands("target_get_field", target, name)?;
    target
        .field(name)
        .ok_or_else(|| unsupported(&format!("'target_get_field' reads no field '{name}'")))
}

/// KGEN's `target_has_feature` over a concrete target and feature name.
pub fn fold_target_has_feature(target: &CtValue, name: &CtValue) -> Result<CtValue, ParamError> {
    let (target, name) = target_operands("target_has_feature", target, name)?;
    Ok(CtValue::Bool(target.has_feature(name)))
}

/// `pop.dtype.to_ui8` over a concrete `DType`: its code, a `UInt8`.
pub fn fold_dtype_to_ui8(dtype: &CtValue) -> Result<CtValue, ParamError> {
    match dtype {
        CtValue::Dtype(dtype) => Ok(CtValue::Simd {
            dtype: Dtype::UInt8,
            lanes: vec![CtLane::Int(i128::from(dtype.code()))],
        }),
        _ => Err(unsupported(
            "'pop.dtype.to_ui8' expects a compile-time 'DType'",
        )),
    }
}

/// `pop.string.size` over a concrete `!kgen.string`: its byte length as an
/// `index`.
pub fn fold_string_size(string: &CtValue) -> Result<CtValue, ParamError> {
    match string {
        CtValue::KgenString(text) => i64::try_from(text.len())
            .map(CtValue::Index)
            .map_err(|_| unsupported("'pop.string.size' reads a string too long for an 'index'")),
        _ => Err(unsupported(&format!(
            "'pop.string.size' expects a compile-time '{}'",
            crate::types::KGEN_STRING_TYPE_SPELLING
        ))),
    }
}

/// `pop.string.address` over a concrete `!kgen.string`: a pointer to its
/// bytes and their nul terminator, frozen as the memory a literal's static
/// bytes are.
pub fn fold_string_address(string: &CtValue) -> Result<CtValue, ParamError> {
    let CtValue::KgenString(text) = string else {
        return Err(unsupported(&format!(
            "'pop.string.address' expects a compile-time '{}'",
            crate::types::KGEN_STRING_TYPE_SPELLING
        )));
    };
    let byte = |byte: u8| {
        Some(CtValue::Simd {
            dtype: Dtype::UInt8,
            lanes: vec![CtLane::Int(i128::from(byte))],
        })
    };
    Ok(CtValue::Pointer {
        memory: Some(text.bytes().chain([0]).map(byte).collect()),
        offset: 0,
    })
}

/// `pop.simd.splat` or `pop.cast` of the one-lane `value` at the type `ty`.
///
/// Its lane converts to `ty`'s dtype ([`CtLane::cast`]), an exact literal
/// by the literal rule ([`CtLane::from_value`]), in every lane of `ty`,
/// held as the host scalar a width-one `Int`, `UInt`, or `Float64` is.
pub fn fold_lane_op(
    op: super::ParamOp,
    ty: &crate::types::Ty,
    value: &CtValue,
) -> Result<CtValue, ParamError> {
    let no_value = || {
        unsupported(&format!(
            "'{}' of '{value}' at '{ty}' has no compile-time value",
            op.name()
        ))
    };
    let (dtype, width) = crate::types::simd_shape(ty.kind()).ok_or_else(no_value)?;
    let lane = match value {
        CtValue::IntLiteral(_) | CtValue::FloatLiteral(_) => CtLane::from_value(value, dtype),
        scalar => CtLane::of_scalar(scalar).map(|lane| lane.cast(dtype)),
    }
    .ok_or_else(no_value)?;
    let width = usize::try_from(width).map_err(|_| no_value())?;
    let vector = CtValue::Simd {
        dtype,
        lanes: vec![lane; width],
    };
    Ok(vector.clone().materialize_as(ty).unwrap_or(vector))
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
        // `pop.div` truncates, and the overflowing `MIN / -1` wraps.
        InfixOp::Div if b == 0 => return Err(zero_divisor(a, b)),
        InfixOp::Div => a.wrapping_div(b),
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
        // `SIMD.__pow__`'s `_powi`, over the exponent cast to `Int32`.
        InfixOp::Pow => match b as i32 {
            exponent if exponent < 0 => match a {
                1 => 1,
                -1 if exponent % 2 == 0 => 1,
                -1 => -1,
                _ => 0,
            },
            exponent => a.wrapping_pow(exponent.unsigned_abs()),
        },
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
        InfixOp::Div if b == 0 => return Err(zero_divisor(a, b)),
        InfixOp::FloorDiv | InfixOp::Mod if b == 0 => 0,
        InfixOp::Div | InfixOp::FloorDiv => a / b,
        InfixOp::Mod => a % b,
        // `SIMD.__pow__`'s `_powi`, over the exponent cast to `Int32`.
        InfixOp::Pow => match b as i32 {
            exponent if exponent < 0 => match a {
                1 => 1,
                u64::MAX if exponent % 2 == 0 => 1,
                u64::MAX => u64::MAX,
                _ => 0,
            },
            exponent => a.wrapping_pow(exponent.unsigned_abs()),
        },
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

/// The bytes a frozen string view addresses, read from its storage as
/// KGEN's interpreter reads it: the view's leaves are a pointer into
/// compile-time memory of `UInt8` slots and a byte length.
fn view_bytes(view: &CtValue) -> Result<Vec<u8>, ParamError> {
    let malformed = || {
        unsupported(&format!(
            "'data_to_str' expects a string view, found {view}"
        ))
    };
    let mut leaves = Vec::new();
    flatten_fields(view, &mut leaves);
    let [
        CtValue::Pointer {
            memory: Some(memory),
            offset,
        },
        CtValue::Int(length),
    ] = leaves.as_slice()
    else {
        return Err(malformed());
    };
    let start = usize::try_from(*offset).map_err(|_| malformed())?;
    let length = usize::try_from(*length).map_err(|_| malformed())?;
    memory
        .get(start..start + length)
        .ok_or_else(malformed)?
        .iter()
        .map(|slot| match slot {
            Some(CtValue::Simd {
                dtype: Dtype::UInt8,
                lanes,
            }) if let [CtLane::Int(byte)] = lanes.as_slice() => {
                u8::try_from(*byte).map_err(|_| malformed())
            }
            _ => Err(malformed()),
        })
        .collect()
}

/// The leaves of a frozen struct value, its nested fields in declaration
/// order.
fn flatten_fields<'a>(value: &'a CtValue, leaves: &mut Vec<&'a CtValue>) {
    match value {
        CtValue::Struct { fields, .. } => {
            for (_, field) in fields {
                flatten_fields(field, leaves);
            }
        }
        _ => leaves.push(value),
    }
}

fn arithmetic(message: &str) -> ParamError {
    ParamError::Arithmetic(message.to_string())
}

/// The pin's rejection of an integer `pop.div` by zero, which it cannot
/// simplify.
fn zero_divisor(a: impl std::fmt::Display, b: impl std::fmt::Display) -> ParamError {
    ParamError::Arithmetic(format!("could not simplify operator div({a}, {b})"))
}

fn unsupported(message: &str) -> ParamError {
    ParamError::Unsupported(message.to_string())
}

/// The target and the `!kgen.string` name a `target_*` read takes.
fn target_operands<'a>(
    operation: &str,
    target: &'a CtValue,
    name: &'a CtValue,
) -> Result<(&'a crate::ct::KgenTarget, &'a str), ParamError> {
    match (target, name) {
        (CtValue::KgenTarget(target), CtValue::KgenString(name)) => Ok((target, name)),
        _ => Err(unsupported(&format!(
            "'{operation}' expects a '!kgen.target' and a '!kgen.string' name"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        fold_comparison, fold_data_to_str, fold_infix, fold_string_address, fold_string_size,
        fold_target_get_field, fold_target_has_feature,
    };
    use crate::ct::{CtLane, CtValue, KgenTarget};
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
    fn data_to_str_reads_each_view_through_its_storage() {
        let view = |text: &str, offset: i64, length: i64| CtValue::Struct {
            name: "StringSpan".to_string(),
            fields: vec![
                (
                    "_data".to_string(),
                    CtValue::Pointer {
                        memory: Some(
                            text.bytes()
                                .map(|byte| Some(lane(Dtype::UInt8, CtLane::Int(byte.into()))))
                                .collect(),
                        ),
                        offset,
                    },
                ),
                ("_size".to_string(), CtValue::Int(length)),
            ],
        };
        assert_eq!(
            fold_data_to_str(
                &view("llvm.", 0, 5),
                &CtValue::Tuple(vec![view("xctpop", 1, 5)])
            )
            .ok(),
            Some(CtValue::KgenString("llvm.ctpop".to_string()))
        );
        assert!(fold_data_to_str(&view("ab", 1, 2), &CtValue::Tuple(Vec::new())).is_err());
        assert!(fold_data_to_str(&CtValue::Int(1), &CtValue::Tuple(Vec::new())).is_err());
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

    #[test]
    fn string_reads_take_the_kgen_strings_bytes() {
        let string = CtValue::KgenString("héllo".to_string());
        assert_eq!(fold_string_size(&string).ok(), Some(CtValue::Index(6)));
        let memory = "héllo\0"
            .bytes()
            .map(|byte| Some(lane(Dtype::UInt8, CtLane::Int(i128::from(byte)))))
            .collect();
        assert_eq!(
            fold_string_address(&string).ok(),
            Some(CtValue::Pointer {
                memory: Some(memory),
                offset: 0,
            })
        );
        assert!(fold_string_size(&CtValue::Str("ab".to_string())).is_err());
    }

    /// `target_get_field` reads the attribute's fields and the triple's
    /// components, `unknown` where the triple has none, as the pin does for
    /// `_unknown`; `target_has_feature` is literal `+name` membership.
    #[test]
    fn target_reads_fold_over_a_closed_target() {
        let name = |text: &str| CtValue::KgenString(text.to_string());
        let target = |triple: &str, features: &str| {
            CtValue::KgenTarget(Box::new(KgenTarget {
                triple: triple.to_string(),
                stdlib_plugin: "default".to_string(),
                arch: "skylake".to_string(),
                features: features.to_string(),
                data_layout: String::new(),
                index_bit_width: 64,
                simd_bit_width: 256,
            }))
        };
        let host = target("x86_64-unknown-linux-gnu", "+avx2,+sse4.1,-avx512f");
        assert_eq!(
            fold_target_get_field(&host, &name("triple_arch")).ok(),
            Some(name("x86_64"))
        );
        assert_eq!(
            fold_target_get_field(&host, &name("os")).ok(),
            Some(name("linux"))
        );
        assert_eq!(
            fold_target_get_field(&host, &name("simd_bit_width")).ok(),
            Some(CtValue::Index(256))
        );
        assert_eq!(
            fold_target_get_field(&host, &name("stdlib_plugin")).ok(),
            Some(name("default"))
        );
        let empty = target("", "");
        assert_eq!(
            fold_target_get_field(&empty, &name("os")).ok(),
            Some(name("unknown"))
        );
        assert_eq!(
            fold_target_get_field(&empty, &name("triple")).ok(),
            Some(name(""))
        );
        assert_eq!(
            fold_target_has_feature(&host, &name("sse4.1")).ok(),
            Some(CtValue::Bool(true))
        );
        for absent in ["avx512f", "sse4", "avx"] {
            assert_eq!(
                fold_target_has_feature(&host, &name(absent)).ok(),
                Some(CtValue::Bool(false))
            );
        }
        assert!(fold_target_get_field(&host, &name("features")).is_err());
        assert!(fold_target_has_feature(&name("x"), &name("avx")).is_err());
    }

    /// A `#kgen.target` attribute's text reads back as the target it spells,
    /// an omitted `stdlib_plugin` reading `default` as at the pin, and one
    /// missing a required field or repeating a field reads as nothing.
    #[test]
    fn target_attribute_text_round_trips() {
        let target = KgenTarget::parse(
            "triple = \"\", arch = \"\", features = \"\", data_layout=\"\",\
             index_bit_width = 0,simd_bit_width = 0",
        )
        .expect("upstream's `_empty_target` parses");
        let text = target.to_string();
        let body = text
            .strip_prefix("#kgen.target<")
            .and_then(|body| body.strip_suffix('>'))
            .expect("the display is the attribute");
        assert_eq!(target.stdlib_plugin, "default");
        assert_eq!(KgenTarget::parse(body), Some(target));
        let cuda = KgenTarget::parse(
            "triple = \"\", stdlib_plugin = \"cuda\", arch = \"\", features = \"\", \
             data_layout=\"\",index_bit_width = 0,simd_bit_width = 0",
        )
        .expect("a spelled `stdlib_plugin` parses");
        assert_eq!(cuda.stdlib_plugin, "cuda");
        let text = cuda.to_string();
        let body = text
            .strip_prefix("#kgen.target<")
            .and_then(|body| body.strip_suffix('>'))
            .expect("the display is the attribute");
        assert_eq!(KgenTarget::parse(body), Some(cuda));
        assert_eq!(
            KgenTarget::parse(&format!("{body}, stdlib_plugin = \"x\"")),
            None
        );
        assert_eq!(KgenTarget::parse("triple = \"\""), None);
        assert_eq!(KgenTarget::parse(&format!("{body}, arch = \"x\"")), None);
    }
}

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
//! Scalar values and recursively materializable tuples/lists/dicts/sets have a
//! runtime literal form; `Type`, `Trait`, `Expr`, `Deferred`,
//! `Marker`, and `RemovedArg` are compile-time-only.

use crate::origin::{Mutability, OriginParamId};
use crate::param_expr::{ParamExpr, ParamRef};
use crate::types::TyKind;
use crate::types::{
    SimdDtype, SimdWidth, Ty, TyArg, dict_elements, list_element, set_element, tuple_elements,
};
use mojito_ast::ast::{Expr, ExprKind, KwArg, ParamArg, Type};
use mojito_common::literal::{FloatLiteral, IntLiteral};
use mojito_common::token::Span;
use std::fmt;

/// The member of a `StringLiteral` that reads its `value` parameter, the
/// literal's KGEN string ([`CtValue::literal_value`]).
pub const LITERAL_VALUE_MEMBER: &str = "value";

/// The storage member of upstream's `SIMD`, `DType`, and `SIMDLength`, whose
/// type is the MLIR type a `__mlir_type[...]` operand reads.
pub const MLIR_VALUE_FIELD: &str = "_mlir_value";

/// One lane of a compile-time SIMD value: integer lanes hold the post-wrap
/// mathematical value (an unsigned lane is non-negative), float lanes their
/// IEEE bits (so equality is structural).
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CtLane {
    Int(i128),
    Float(u64),
    Bool(bool),
}

impl CtLane {
    /// One element's lane of `dtype` under upstream's `*elems:
    /// Scalar[dtype]`, or `None` when the element does not convert to the
    /// lane's scalar implicitly.
    ///
    /// A literal materializes by the literal rule ([`Self::of_int_literal`],
    /// [`Self::of_float_literal`]), a one-lane `Scalar[dtype]` (a host `Int`,
    /// `UInt`, or `Float64` at its own dtype) is its own lane, and a `Bool` is
    /// a mask's lane. Any other value is the cast only a single argument
    /// takes ([`Self::from_value`]).
    pub fn from_element(value: &CtValue, dtype: mojito_ast::ast::Dtype) -> Option<Self> {
        use mojito_ast::ast::Dtype;
        match (dtype, value) {
            (_, CtValue::IntLiteral(_) | CtValue::FloatLiteral(_) | CtValue::Simd { .. })
            | (Dtype::Bool, CtValue::Bool(_))
            | (Dtype::Int, CtValue::Int(_))
            | (Dtype::UInt, CtValue::UInt(_))
            | (Dtype::Float64, CtValue::Float(_)) => Self::from_value(value, dtype),
            _ => None,
        }
    }

    /// The lane of `dtype` one value converts to as the single argument of
    /// a construction (upstream's cast from `SIMD[other_dtype,
    /// Self.length]` over a splatted scalar), or `None` when it does not fit
    /// the lane (a float into an integer lane, a vector of another dtype).
    ///
    /// A literal materializes by the literal rule, a one-lane value of the
    /// dtype is its own lane, and a host `Int`, `UInt`, or `Float64` wraps or
    /// rounds into the lane.
    pub fn from_value(value: &CtValue, dtype: mojito_ast::ast::Dtype) -> Option<Self> {
        use mojito_ast::ast::Dtype;
        match (dtype, value) {
            (_, CtValue::IntLiteral(literal)) => Self::of_int_literal(literal, dtype),
            (_, CtValue::FloatLiteral(literal)) => Self::of_float_literal(literal, dtype),
            (_, CtValue::Simd { dtype: own, lanes }) if *own == dtype => match lanes.as_slice() {
                [lane] => Some(lane.clone()),
                _ => None,
            },
            (Dtype::Bool, CtValue::Bool(value)) => Some(Self::Bool(*value)),
            (Dtype::Bool, _) => None,
            (Dtype::Float16 | Dtype::Float32 | Dtype::Float64, value) => {
                let value = match value {
                    CtValue::Float(bits) => f64::from_bits(*bits),
                    CtValue::Int(value) => *value as f64,
                    CtValue::UInt(value) => *value as f64,
                    _ => return None,
                };
                Some(Self::Float(dtype.round_lane(value).to_bits()))
            }
            (integer, value) => {
                let wide: i128 = match value {
                    CtValue::Int(value) => i128::from(*value),
                    CtValue::UInt(value) => i128::from(*value),
                    CtValue::Bool(value) => i128::from(*value),
                    _ => return None,
                };
                Some(Self::Int(wrap_lane(integer, wide)))
            }
        }
    }

    /// The one lane of a scalar value: a host `Int`, `UInt`, `Float64`, or
    /// `Bool`, or a one-lane vector. `None` for any other value.
    pub fn of_scalar(value: &CtValue) -> Option<Self> {
        Some(match value {
            CtValue::Int(n) => Self::Int(i128::from(*n)),
            CtValue::UInt(n) => Self::Int(i128::from(*n)),
            CtValue::Float(bits) => Self::Float(*bits),
            CtValue::Bool(b) => Self::Bool(*b),
            CtValue::Simd { lanes, .. } if let [lane] = lanes.as_slice() => lane.clone(),
            _ => return None,
        })
    }

    /// This lane as one of `dtype`, as upstream's `pop.cast` converts a
    /// scalar: an integer wraps or rounds into the lane, a float truncates
    /// toward zero then wraps or rounds, a `bool` is 0/1, and into a `bool`
    /// lane only the low bit is kept, as the VM's `simd_cast` converts.
    #[must_use]
    pub fn cast(&self, dtype: mojito_ast::ast::Dtype) -> Self {
        if dtype == mojito_ast::ast::Dtype::Bool {
            return Self::Bool(match self {
                Self::Int(n) => n & 1 != 0,
                Self::Float(bits) => (f64::from_bits(*bits).trunc() as i128) & 1 != 0,
                Self::Bool(b) => *b,
            });
        }
        match (self, dtype.is_float()) {
            (Self::Int(n), false) => Self::Int(wrap_lane(dtype, *n)),
            (Self::Int(n), true) => Self::Float(dtype.round_lane(*n as f64).to_bits()),
            (Self::Float(bits), false) => {
                Self::Int(wrap_lane(dtype, f64::from_bits(*bits).trunc() as i128))
            }
            (Self::Float(bits), true) => {
                Self::Float(dtype.round_lane(f64::from_bits(*bits)).to_bits())
            }
            (Self::Bool(b), false) => Self::Int(i128::from(*b)),
            (Self::Bool(b), true) => Self::Float(f64::from(u8::from(*b)).to_bits()),
        }
    }

    /// Materialize an exact integer literal into one lane of `dtype`, as
    /// upstream's `SIMD.__init__(IntLiteral)` does: an integer lane wraps at
    /// the dtype's width, a float lane rounds the exact literal once, and a
    /// `bool` lane is `None`.
    pub fn of_int_literal(literal: &IntLiteral, dtype: mojito_ast::ast::Dtype) -> Option<Self> {
        if dtype.is_float() {
            return Self::of_float_literal(&FloatLiteral::from_int(literal), dtype);
        }
        let bits = dtype.bit_width();
        match dtype {
            mojito_ast::ast::Dtype::Bool => None,
            _ if dtype.is_unsigned() => literal.wrapping_unsigned(bits).map(i128::from),
            _ => literal.wrapping_signed(bits).map(i128::from),
        }
        .map(Self::Int)
    }

    /// Materialize an exact float literal into one lane of a float `dtype`,
    /// correctly rounded once; `None` for an integer or `bool` lane.
    pub fn of_float_literal(literal: &FloatLiteral, dtype: mojito_ast::ast::Dtype) -> Option<Self> {
        dtype
            .is_float()
            .then(|| dtype.float_literal_lane(literal))
            .flatten()
            .map(|lane| Self::Float(lane.to_bits()))
    }

    /// One lane of a `#kgen.simd<…>` attribute of `dtype`, spelled as the
    /// pin reads it: `true` or `false` for a `bool` lane, a decimal integer
    /// for an integer lane, and a quoted decimal float, `"inf"`, `"-inf"`,
    /// or `"nan"` for a float lane. `None` for any other spelling.
    pub fn parse_attribute(text: &str, dtype: mojito_ast::ast::Dtype) -> Option<Self> {
        let text = text.trim();
        if dtype == mojito_ast::ast::Dtype::Bool {
            return match text {
                "true" => Some(Self::Bool(true)),
                "false" => Some(Self::Bool(false)),
                _ => None,
            };
        }
        if dtype.is_float() {
            let text = text.strip_prefix('"')?.strip_suffix('"')?;
            let (negative, magnitude) = text
                .strip_prefix('-')
                .map_or((false, text), |magnitude| (true, magnitude));
            return match FloatLiteral::parse_decimal(magnitude) {
                Some(literal) => {
                    Self::of_float_literal(&if negative { literal.neg() } else { literal }, dtype)
                }
                None => matches!(magnitude, "inf" | "nan")
                    .then(|| text.parse::<f64>().ok())
                    .flatten()
                    .map(|value| Self::Float(dtype.round_lane(value).to_bits())),
            };
        }
        let (negative, magnitude) = text
            .strip_prefix('-')
            .map_or((false, text), |magnitude| (true, magnitude));
        let literal = IntLiteral::parse_radix(magnitude, 10)?;
        Self::of_int_literal(&if negative { literal.neg() } else { literal }, dtype)
    }
}

/// Why a compile-time `SIMD[dtype, width](values…)` construction has no
/// value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimdConstructionError {
    /// Neither one value (a splat) nor one value per lane.
    Arity,
    /// A value does not fit a lane of the dtype.
    Lane,
}

/// The lanes of `SIMD[dtype, width](values…)`.
///
/// One value converts by [`CtLane::from_value`] and splats across the
/// width, and `width > 1` values give one lane each (upstream's `*elems:
/// Scalar[dtype]`), each by [`CtLane::from_element`].
///
/// The checker's constraint evaluator and `sccp`'s `simd.make` fold build a
/// construction's lanes by this one rule.
pub fn simd_construction_lanes(
    dtype: mojito_ast::ast::Dtype,
    width: usize,
    values: &[CtValue],
) -> Result<Vec<CtLane>, SimdConstructionError> {
    let lane = |lane: Option<CtLane>| lane.ok_or(SimdConstructionError::Lane);
    match values {
        [value] => Ok(vec![lane(CtLane::from_value(value, dtype))?; width]),
        _ if values.len() == width => values
            .iter()
            .map(|value| lane(CtLane::from_element(value, dtype)))
            .collect(),
        _ => Err(SimdConstructionError::Arity),
    }
}

/// Wrap an integer to the mathematical value of a `dtype` lane (two's
/// complement for signed lanes, modulo 2^bits for unsigned ones).
pub fn wrap_lane(dtype: mojito_ast::ast::Dtype, value: i128) -> i128 {
    use mojito_ast::ast::Dtype;
    match dtype {
        Dtype::Int | Dtype::Int64 => i128::from(value as i64),
        Dtype::Int8 => i128::from(value as i8),
        Dtype::Int16 => i128::from(value as i16),
        Dtype::Int32 => i128::from(value as i32),
        Dtype::UInt8 => i128::from(value as u8),
        Dtype::UInt16 => i128::from(value as u16),
        Dtype::UInt32 => i128::from(value as u32),
        Dtype::UInt | Dtype::UInt64 => i128::from(value as u64),
        Dtype::Float16 | Dtype::Float32 | Dtype::Float64 | Dtype::Bool => value,
    }
}

/// One integer lane of `op` over two lanes of `dtype`, as upstream's `pop`
/// arithmetic computes it.
///
/// `None` is an operator integer lanes do not define, or `/` by a zero
/// lane, which traps. `+ - *` wrap, `/` truncates (upstream's `pop.div`,
/// wrapping the overflowing `MIN / -1`), `//` and `%` floor and give a
/// zero lane for a zero divisor, and a shift masks
/// its amount to the lane width (a logical right shift for an unsigned
/// lane). The VM's `simd_binop` and the compile-time
/// fold both compute lanes through this one rule.
pub fn int_lane_binop(
    op: mojito_ast::ast::InfixOp,
    dtype: mojito_ast::ast::Dtype,
    a: i128,
    b: i128,
) -> Option<i128> {
    use mojito_ast::ast::InfixOp;
    let lane = match op {
        InfixOp::Add => a.wrapping_add(b),
        InfixOp::Sub => a.wrapping_sub(b),
        // Two 64-bit lanes can overflow the i128 product; wrapping to the
        // dtype width preserves the wrapped product.
        InfixOp::Mul => a.wrapping_mul(b),
        InfixOp::Div if b == 0 => return None,
        InfixOp::Div => a.wrapping_div(b),
        // Upstream's integer `SIMD.__floordiv__`/`__mod__`: floor semantics,
        // and a zero divisor selects a zero lane rather than trapping.
        InfixOp::FloorDiv | InfixOp::Mod if b == 0 => 0,
        InfixOp::FloorDiv => {
            let q = a.wrapping_div(b);
            if (a % b != 0) && ((a < 0) != (b < 0)) {
                q - 1
            } else {
                q
            }
        }
        InfixOp::Mod => {
            let r = a.wrapping_rem(b);
            if r != 0 && ((r < 0) != (b < 0)) {
                r + b
            } else {
                r
            }
        }
        InfixOp::BitAnd => a & b,
        InfixOp::BitOr => a | b,
        InfixOp::BitXor => a ^ b,
        InfixOp::Shl | InfixOp::Shr => {
            let amount = (b as u32) & (dtype.bit_width() - 1);
            match op {
                InfixOp::Shl => a << amount,
                _ if dtype.is_unsigned() => ((a as u128) >> amount) as i128,
                _ => a >> amount,
            }
        }
        _ => return None,
    };
    Some(wrap_lane(dtype, lane))
}

/// One float lane of `op` over two lanes of `dtype`, or `None` for an
/// operator other than `+ - * / //`.
///
/// The lane is computed at double precision and rounded once to the lane,
/// which is exact IEEE arithmetic at `float32` and `float16`. `//` is
/// upstream's `pop.floordiv`: the floor of the lane-rounded quotient.
pub fn float_lane_binop(
    op: mojito_ast::ast::InfixOp,
    dtype: mojito_ast::ast::Dtype,
    a: f64,
    b: f64,
) -> Option<f64> {
    use mojito_ast::ast::InfixOp;
    let lane = match op {
        InfixOp::Add => a + b,
        InfixOp::Sub => a - b,
        InfixOp::Mul => a * b,
        InfixOp::Div => a / b,
        InfixOp::FloorDiv => dtype.round_lane(a / b).floor(),
        _ => return None,
    };
    Some(dtype.round_lane(lane))
}

/// A lane comparison under `pop.cmp`'s ordered predicates, or `None` for an
/// operator that is not one.
///
/// An unordered pair (a `NaN` lane) compares true under no predicate, `ne`
/// included (`fcmp one`); upstream's `!=` is `not ==`, which a `NaN` makes
/// true.
pub fn lane_compare<T: PartialOrd + Copy>(
    op: mojito_ast::ast::InfixOp,
    a: T,
    b: T,
) -> Option<bool> {
    use mojito_ast::ast::InfixOp;
    use std::cmp::Ordering::{Equal, Greater, Less};
    let order = a.partial_cmp(&b);
    Some(match op {
        InfixOp::Eq => order == Some(Equal),
        InfixOp::Ne => matches!(order, Some(Less | Greater)),
        InfixOp::Lt => order == Some(Less),
        InfixOp::Gt => order == Some(Greater),
        InfixOp::Le => matches!(order, Some(Less | Equal)),
        InfixOp::Ge => matches!(order, Some(Greater | Equal)),
        _ => return None,
    })
}

/// One `bool` lane of `op`, or `None` for an operator `bool` lanes do not
/// define. `False` orders below `True`, as the `i1` unsigned predicates do.
pub fn bool_lane_binop(op: mojito_ast::ast::InfixOp, a: bool, b: bool) -> Option<bool> {
    use mojito_ast::ast::InfixOp;
    match op {
        InfixOp::BitAnd => Some(a & b),
        InfixOp::BitOr => Some(a | b),
        InfixOp::BitXor => Some(a ^ b),
        _ => lane_compare(op, a, b),
    }
}

/// The target of a built-in scalar conversion: `Int(x)`, `UInt(x)`,
/// `Float64(x)`, or `Bool(x)`, the type upstream's inlined `pop.cast` (or
/// `Bool.__init__`'s compare) produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ScalarConversion {
    Int,
    UInt,
    Float64,
    Bool,
}

impl ScalarConversion {
    /// The conversion to the scalar type spelled `name`, or `None` for any
    /// other spelling.
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "Int" => Self::Int,
            "UInt" => Self::UInt,
            "Float64" => Self::Float64,
            "Bool" => Self::Bool,
            _ => return None,
        })
    }

    /// The conversion producing `ty`, or `None` for any other type.
    pub fn to(ty: &Ty) -> Option<Self> {
        Some(match ty.kind() {
            TyKind::INT => Self::Int,
            TyKind::UINT => Self::UInt,
            TyKind::FLOAT64 => Self::Float64,
            TyKind::Bool => Self::Bool,
            _ => return None,
        })
    }

    /// The spelling of the type this conversion produces.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Int => "Int",
            Self::UInt => "UInt",
            Self::Float64 => "Float64",
            Self::Bool => "Bool",
        }
    }

    /// The type this conversion produces.
    pub fn ty(self) -> Ty {
        match self {
            Self::Int => Ty::int(),
            Self::UInt => Ty::uint(),
            Self::Float64 => Ty::float64(),
            Self::Bool => Ty::bool(),
        }
    }

    /// Whether a value of type `ty` converts by this conversion as a cast:
    /// a numeric scalar or literal, a `Bool`, a width-one SIMD value, and
    /// to `Bool` the MLIR builtin `i1` (upstream's `@implicit
    /// Bool.__init__(value: __mlir_type.i1)`).
    pub fn casts_from(self, ty: &Ty) -> bool {
        matches!(
            ty.kind(),
            TyKind::INT
                | TyKind::UINT
                | TyKind::FLOAT64
                | TyKind::IntLiteral
                | TyKind::FloatLiteral
                | TyKind::Bool
        ) || crate::types::is_scalar_simd(ty.kind())
            || (self == Self::Bool && *ty.kind() == TyKind::I1)
    }
}

/// Convert one scalar lane to `target`, as upstream's inlined
/// `SIMD.__init__`/`Bool.__init__` do.
///
/// An integer converts by wrapping (or rounding to `Float64`), a float
/// truncates toward zero with Rust's saturating casts (`fptosi.sat`), a
/// `bool` is 0/1, and `Bool(x)` is `__bool__`'s ordered `pop.cmp ne` with
/// zero, so `NaN` is `False`.
pub fn convert_lane(lane: &CtLane, target: ScalarConversion) -> CtValue {
    match (lane, target) {
        (CtLane::Int(n), ScalarConversion::Int) => CtValue::Int(*n as i64),
        (CtLane::Int(n), ScalarConversion::UInt) => CtValue::UInt(*n as u64),
        (CtLane::Int(n), ScalarConversion::Float64) => CtValue::Float((*n as f64).to_bits()),
        (CtLane::Int(n), ScalarConversion::Bool) => CtValue::Bool(*n != 0),
        (CtLane::Float(bits), target) => {
            let x = f64::from_bits(*bits);
            match target {
                ScalarConversion::Int => CtValue::Int(x as i64),
                ScalarConversion::UInt => CtValue::UInt(x as u64),
                ScalarConversion::Float64 => CtValue::Float(*bits),
                ScalarConversion::Bool => CtValue::Bool(x != 0.0 && !x.is_nan()),
            }
        }
        (CtLane::Bool(b), ScalarConversion::Int) => CtValue::Int(i64::from(*b)),
        (CtLane::Bool(b), ScalarConversion::UInt) => CtValue::UInt(u64::from(*b)),
        (CtLane::Bool(b), ScalarConversion::Float64) => {
            CtValue::Float(f64::from(u8::from(*b)).to_bits())
        }
        (CtLane::Bool(b), ScalarConversion::Bool) => CtValue::Bool(*b),
    }
}

/// A compile-time value.
///
/// Scalar values drive folding; `Tuple`/`List` are compile-time
/// collections; `Type` carries a semantic type for associated
/// comptime members; `Expr` is a symbolic parameter expression while a generic
/// body is being checked.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CtValue {
    /// An already-materialized machine `Int` compile-time value. Compiler
    /// generated indices, lengths, and value parameters use this variant.
    Int(i64),
    UInt(u64),
    /// A value of the MLIR builtin `index` type — upstream's `N : index`
    /// integer attribute, what `Int.__mlir_index__()` (`pop.cast_to_builtin`)
    /// yields at compile time. It is the 64-bit host target's `index`, and
    /// displays bare (`S[3]`), as the pin prints it.
    Index(i64),
    /// The bits of an already-materialized `Float64` compile-time value.
    Float(u64),
    /// An arbitrary-precision integer literal which has not yet been
    /// materialized into a fixed-width scalar.
    IntLiteral(IntLiteral),
    /// An exact finite floating literal which has not yet been materialized.
    FloatLiteral(FloatLiteral),
    Bool(bool),
    /// The `NoneType` value — upstream's `None` parameter (a
    /// `NoneType = None` default, an `OptionalReg[T] = None` member). It
    /// displays as `None`.
    None,
    Str(String),
    Tuple(Vec<Self>),
    List(Vec<Self>),
    /// A compile-time dictionary display (`comptime M = {"a": 1}`):
    /// insertion-ordered entries deduplicated by structural key equality (a
    /// later duplicate key replaces the value in place, as `Dict.__setitem__`
    /// does). `spelling` is the explicit nominal type when one was given — an
    /// annotation (`comptime E: Dict[String, Int] = {}`) or the explicit
    /// literal constructor (`Dict[K, V, H](keys, values, None)`) — so
    /// materialization spells that constructor; `None` is the bare display,
    /// which the checker types with the default hasher.
    Dict {
        spelling: Option<Ty>,
        entries: Vec<(Self, Self)>,
    },
    /// A compile-time set display (`comptime S = {1, 2, 3}`); see `Dict`.
    Set {
        spelling: Option<Ty>,
        elements: Vec<Self>,
    },
    /// A `DType.<dt>` compile-time value — the binding of a `[dtype: DType]`
    /// value parameter. Materializes as the member spelling, which type
    /// resolution already accepts inside `SIMD[...]`/`Scalar[...]` brackets.
    Dtype(mojito_ast::ast::Dtype),
    /// A compile-time SIMD vector — the binding of a `[key: SIMD[DType.d, w]]`
    /// value parameter (`AHasher[key: U256]`). Lanes are already wrapped to
    /// `dtype`; it displays as upstream's parameter rendering
    /// (`[0, 0, 0, 0] : SIMD[DType.uint64, 4]`) and materializes as the
    /// explicit `SIMD[DType.d, w](lanes...)` construction.
    Simd {
        dtype: mojito_ast::ast::Dtype,
        lanes: Vec<CtLane>,
    },
    /// A frozen struct instance (declaration-ordered fields) — the binding of
    /// a struct-typed value parameter such as `[e: Extent]`. Freezing is
    /// restricted to structs constructible fieldwise from recursively
    /// freezable fields, so materialization is always the fieldwise
    /// construction call.
    Struct {
        #[serde(serialize_with = "crate::symbol_ref::serialize")]
        name: String,
        fields: Vec<(String, Self)>,
    },
    /// A frozen tagged union — an `enum`'s storage, or a `Variant`'s —
    /// over `alternatives`, holding the payload of alternative `index`, as
    /// upstream's parameter domain holds a `!kgen.variant` value; `None`
    /// for an alternative of type `NoneType`, a case without a payload. An
    /// enum's frozen case (`Token.eof`) is the struct whose one field this
    /// is.
    Variant {
        alternatives: Vec<Ty>,
        index: usize,
        payload: Option<Box<Self>>,
    },
    /// The frozen allocation a pointer field of a compile-time value points
    /// into — what VM compile-time evaluation leaves in its heap behind an
    /// `Optional`'s or a `List`'s storage pointer — and the pointer's
    /// element offset into it: one entry per slot, `None` for a slot never
    /// written or moved out (an empty `Optional`'s one slot, a `List`'s
    /// spare capacity); a dangling pointer has no memory. Each pointer
    /// freezes its own copy of its allocation, and a backend materializes
    /// the value as fresh memory written from the slots, as upstream's
    /// `materialize_into` copies the parameter domain's memory out.
    Pointer {
        memory: Option<Vec<Option<Self>>>,
        offset: i64,
    },
    /// A reference into compile-time memory, frozen as the value it reads —
    /// upstream's `#lit.store_to_mem<value>`, a `ref[ComptimeOrigin]` to a
    /// parameter-domain value a compile-time evaluation borrowed (the
    /// collection a `d.keys()` iterator walks). Compile-time memory is never
    /// destroyed, so the referent freezes by value, as it reads when the
    /// evaluation ends; each reference freezes its own copy, as
    /// [`Self::Pointer`]'s memory does.
    StoreToMem(Box<Self>),
    Type(Ty),
    /// A trait, or a `&` composition of traits, used as a value — upstream's
    /// `!lit.trait<…>`, whose type is the trait metatype
    /// ([`TyKind::AnyTrait`]). The list is the canonical trait set
    /// ([`crate::types::trait_set`]); it displays as `A & B`.
    Trait(Vec<String>),
    /// A KGEN string attribute — upstream's `#kgen.string`, the `value`
    /// parameter of a `StringLiteral` ([`Self::literal_value`]), whose type
    /// is [`TyKind::KgenString`]. It displays as `"text"`, as the pin prints
    /// `Tag["Some"]`.
    KgenString(String),
    /// An untyped MLIR attribute — upstream's `` __mlir_attr.`#pop.fmf<fast>` ``,
    /// held as its text after the backtick, whose type is
    /// [`TyKind::KgenDeferred`]. It displays as its text.
    MlirAttr(String),
    /// A compilation target — upstream's `#kgen.target<…>` attribute, the
    /// `_mlir_value` of a `CompilationTarget`, whose type is
    /// [`TyKind::KgenTarget`]. It displays as the attribute.
    KgenTarget(Box<KgenTarget>),
    /// A POP array attribute — upstream's `#pop.array<…>`, a typed attribute
    /// whose type is `!pop.array<n, element>` ([`TyKind::PopArray`]) with
    /// `n` the element count: a `` __mlir_attr.`#pop.array<3, 2, 1, 0> :
    /// !pop.array<4, index>` `` mask, or a frozen `StaticTuple`'s storage.
    /// It displays as the attribute.
    PopArray {
        element: Ty,
        elements: Vec<Self>,
    },
    /// A closed generator — upstream's `#kgen.gen<body>`, the value of a
    /// parameterized `comptime` declaration read unapplied (`Twice`,
    /// `A.get`). Its body names its own parameters and nothing else; its
    /// type is [`TyKind::Generator`]. It displays as `__gen[x: Int] x * 2`.
    Generator(Box<GeneratorValue>),
    /// A residual parameter expression: a declared parameter, or an operator
    /// over one, while a generic declaration is checked. Never a constant — a
    /// folded expression is its ordinary concrete variant
    /// ([`ParamExpr::into_value`]).
    Expr(ParamExpr),
    /// The slot of a declared value parameter whose value arrives later and
    /// takes no part in generic identity: a callable-value parameter the VM
    /// reifies under the binder's spelling, or a value argument only the
    /// elaborator can fold. It names the binder whose slot it fills, is not a
    /// reference to a parameter in scope, and never enters a specialization
    /// key.
    Deferred(ParamRef),
    /// An elaborator-private classification of a name. It stands for no
    /// parameter and no value.
    Marker(CtMarker),
    /// A struct argument that mentions a parameter the instance's generator
    /// removed — upstream's `_REMOVED_ARG`. It stands in a value or a type
    /// slot alike and is closed: an instance's erased struct type
    /// (`Box[_REMOVED_ARG]`) is laid out from fields that read no erased
    /// argument, so it agrees with every instance of its template that
    /// matches the kept arguments. It never reaches a runtime value.
    RemovedArg,
}

/// The stdlib plugin a target that names none selects, upstream's
/// `DefaultPlugin.name`.
pub const DEFAULT_STDLIB_PLUGIN: &str = "default";

/// Upstream's `#kgen.target<…>` attribute: a compilation target's fields.
///
/// `#kgen.param.expr<target_get_field>` and
/// `#kgen.param.expr<target_has_feature>` read them ([`Self::field`],
/// [`Self::has_feature`]). The `opaque_plugin` payload only the GPU
/// targets spell is not read.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct KgenTarget {
    pub triple: String,
    /// The name of the stdlib plugin the target selects, `default` where
    /// the attribute omits it.
    pub stdlib_plugin: String,
    /// The CPU name (`skylake`, `x86-64`).
    pub arch: String,
    /// The effective feature list, each item `+name` or `-name`, joined
    /// with `,`.
    pub features: String,
    pub data_layout: String,
    pub index_bit_width: i64,
    pub simd_bit_width: i64,
}

impl KgenTarget {
    /// The `#kgen.target` attribute's field names, in its spelling's order.
    pub const FIELDS: [&'static str; 6] = [
        "triple",
        "arch",
        "features",
        "data_layout",
        "index_bit_width",
        "simd_bit_width",
    ];

    /// The fields a `#kgen.target` attribute may omit, each with the value
    /// it then reads.
    pub const OPTIONAL_FIELDS: [(&'static str, &'static str); 1] =
        [("stdlib_plugin", DEFAULT_STDLIB_PLUGIN)];

    /// The value `target_get_field` reads for `name`: a `!kgen.string`
    /// for `triple`, `arch`, and the triple's `triple_arch` and `os`
    /// components (`unknown` where the triple has none), and an `index`
    /// for the bit widths. `None` for any other name.
    pub fn field(&self, name: &str) -> Option<CtValue> {
        let component = |at: usize| {
            self.triple
                .split('-')
                .nth(at)
                .filter(|part| !part.is_empty())
                .unwrap_or("unknown")
                .to_string()
        };
        Some(match name {
            "triple" => CtValue::KgenString(self.triple.clone()),
            "stdlib_plugin" => CtValue::KgenString(self.stdlib_plugin.clone()),
            "arch" => CtValue::KgenString(self.arch.clone()),
            "triple_arch" => CtValue::KgenString(component(0)),
            "os" => CtValue::KgenString(component(2)),
            "index_bit_width" => CtValue::Index(self.index_bit_width),
            "simd_bit_width" => CtValue::Index(self.simd_bit_width),
            _ => return None,
        })
    }

    /// Whether `target_get_field` reads a field `name` ([`Self::field`]).
    pub fn reads_field(name: &str) -> bool {
        matches!(
            name,
            "triple"
                | "stdlib_plugin"
                | "arch"
                | "triple_arch"
                | "os"
                | "index_bit_width"
                | "simd_bit_width"
        )
    }

    /// Whether `target_get_field` reads `name` as an `index`, not a
    /// `!kgen.string`.
    pub fn is_index_field(name: &str) -> bool {
        matches!(name, "index_bit_width" | "simd_bit_width")
    }

    /// Whether the feature list holds `+name`, as `target_has_feature`
    /// answers: literal membership, with no implication between features.
    pub fn has_feature(&self, name: &str) -> bool {
        self.features
            .split(',')
            .any(|feature| feature.strip_prefix('+') == Some(name))
    }

    /// Parse the body of a `#kgen.target<…>` attribute: each field once,
    /// `name = value`, the strings quoted and the widths integers, an
    /// optional one ([`Self::OPTIONAL_FIELDS`]) taking its default when
    /// omitted.
    pub fn parse(body: &str) -> Option<Self> {
        let mut target = Self {
            triple: String::new(),
            stdlib_plugin: DEFAULT_STDLIB_PLUGIN.to_string(),
            arch: String::new(),
            features: String::new(),
            data_layout: String::new(),
            index_bit_width: 0,
            simd_bit_width: 0,
        };
        let mut seen = Vec::new();
        let mut rest = body.trim();
        while !rest.is_empty() {
            let (name, after) = rest.split_once('=')?;
            let name = name.trim();
            let after = after.trim_start();
            let (value, after) = match after.strip_prefix('"') {
                Some(quoted) => {
                    let end = quoted.find('"')?;
                    (&quoted[..end], &quoted[end + 1..])
                }
                None => after.split_at(after.find(',').unwrap_or(after.len())),
            };
            let optional = Self::OPTIONAL_FIELDS
                .iter()
                .any(|(optional, _)| *optional == name);
            if !(Self::FIELDS.contains(&name) || optional) || seen.contains(&name) {
                return None;
            }
            seen.push(name);
            match name {
                "triple" => target.triple = value.to_string(),
                "stdlib_plugin" => target.stdlib_plugin = value.to_string(),
                "arch" => target.arch = value.to_string(),
                "features" => target.features = value.to_string(),
                "data_layout" => target.data_layout = value.to_string(),
                "index_bit_width" => target.index_bit_width = value.trim().parse().ok()?,
                _ => target.simd_bit_width = value.trim().parse().ok()?,
            }
            let after = after.trim_start();
            rest = after.strip_prefix(',').unwrap_or(after).trim_start();
        }
        Self::FIELDS
            .iter()
            .all(|field| seen.contains(field))
            .then_some(target)
    }
}

impl fmt::Display for KgenTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "#kgen.target<triple = {:?}, stdlib_plugin = {:?}, arch = {:?}, features = {:?}, \
             data_layout = {:?}, index_bit_width = {}, simd_bit_width = {}>",
            self.triple,
            self.stdlib_plugin,
            self.arch,
            self.features,
            self.data_layout,
            self.index_bit_width,
            self.simd_bit_width
        )
    }
}

/// A generator's input parameters over the body applying them yields,
/// upstream's `GeneratorAttr { inputParamTypes, body }`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct GeneratorValue {
    pub params: Vec<crate::types::ParamDecl>,
    pub body: ParamExpr,
}

impl GeneratorValue {
    /// The generator type this value has: its parameters over its body's
    /// type, a type-valued body yielding the type of all types.
    pub fn ty(&self) -> Ty {
        let result = self
            .body
            .meta()
            .value_ty()
            .unwrap_or_else(|| TyKind::TypeType.into());
        Ty::generator(self.params.clone(), result)
    }
}

/// What the elaborator knows about a name that has no compile-time value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CtMarker {
    /// A runtime local: a parameter or a declared variable.
    RuntimeLocal,
    /// A declared struct name.
    TypeName,
    /// An origin parameter of the struct being walked, by its
    /// declaration-order identity and the permission it grants.
    TupleOrigin {
        id: OriginParamId,
        mutability: Mutability,
    },
    /// A module constant whose initializer applies a callable, asks a
    /// layout, or reads such a constant: its identity is the application
    /// (decision D3), which the check binds and the elaborator below MIR
    /// evaluates on first demand. It has no value above the check.
    Applied,
}

impl fmt::Display for CtMarker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeLocal => f.write_str("$local"),
            Self::TypeName => f.write_str("$type"),
            Self::Applied => f.write_str("$applied"),
            Self::TupleOrigin { id, mutability } => {
                let permission = match mutability {
                    Mutability::Immutable => "imm",
                    Mutability::Mutable => "mut",
                    Mutability::Param(_) => "param",
                };
                write!(f, "$tuple-origin:{}:{permission}", id.0)
            }
        }
    }
}

impl CtValue {
    /// The `value` parameter of the `StringLiteral` this compile-time value
    /// is, as upstream's `StringLiteral[value: __mlir_type.`!kgen.string`]`
    /// reads it: the literal's KGEN string. `None` for any other value.
    pub fn literal_value(&self) -> Option<Self> {
        match self {
            Self::Str(text) => Some(Self::KgenString(text.clone())),
            _ => None,
        }
    }

    /// The type of a `#pop.array` attribute: `!pop.array<n, element>` with
    /// `n` its element count. `None` for any other value.
    pub fn pop_array_type(&self) -> Option<Ty> {
        matches!(self, Self::PopArray { .. })
            .then(|| {
                crate::param_expr::MetaTy::of_value(self)
                    .as_value()
                    .cloned()
            })
            .flatten()
    }

    /// The elements a compile-time collection read sees, in order: a list's
    /// or a set's elements, a dictionary's keys, and a bound value pack's
    /// elements; `None` for any other value. Compile-time indexing, `len`,
    /// and parameter-list reads share this one rule.
    pub fn comptime_iteration_elements(&self) -> Option<Vec<Self>> {
        match self {
            Self::Tuple(values)
            | Self::List(values)
            | Self::Set {
                elements: values, ..
            } => Some(values.clone()),
            Self::Dict { entries, .. } => {
                Some(entries.iter().map(|(key, _)| key.clone()).collect())
            }
            Self::Struct { .. } => self.frozen_collection_elements(),
            _ => None,
        }
    }

    /// The elements a frozen bundled collection holds, in insertion order:
    /// an `Array`'s or a `List`'s initialized storage, a `Set`'s items, and
    /// a `Dict`'s entry keys. `None` for any other value, and for a
    /// collection whose storage is not fully initialized.
    fn frozen_collection_elements(&self) -> Option<Vec<Self>> {
        use crate::types::{ARRAY_TYPE_NAME, DICT_TYPE_NAME, LIST_TYPE_NAME, SET_TYPE_NAME};
        let Self::Struct { name, fields } = self else {
            return None;
        };
        let field = |wanted: &str| {
            fields
                .iter()
                .find(|(field, _)| field == wanted)
                .map(|(_, value)| value)
        };
        let storage = |size: &str| {
            if field(size) == Some(&Self::Int(0)) {
                return Some(Vec::new());
            }
            let (
                Some(Self::Pointer {
                    memory: Some(slots),
                    offset,
                }),
                Some(Self::Int(size)),
            ) = (field("data"), field(size))
            else {
                return None;
            };
            let start = usize::try_from(*offset).ok()?;
            let end = start.checked_add(usize::try_from(*size).ok()?)?;
            slots.get(start..end)?.iter().cloned().collect()
        };
        if crate::types::is_nominal_instance(name, ARRAY_TYPE_NAME) {
            storage("_size")
        } else if crate::types::is_nominal_instance(name, LIST_TYPE_NAME) {
            storage("size")
        } else if crate::types::is_nominal_instance(name, SET_TYPE_NAME) {
            field("items")?.frozen_collection_elements()
        } else if crate::types::is_nominal_instance(name, DICT_TYPE_NAME) {
            field("entries")?
                .frozen_collection_elements()?
                .into_iter()
                .map(|entry| match entry {
                    Self::Struct { fields, .. } => fields
                        .into_iter()
                        .find(|(field, _)| field == "key")
                        .map(|(_, key)| key),
                    _ => None,
                })
                .collect()
        } else {
            None
        }
    }

    /// A compile-time dictionary from display-ordered entries: a repeated key
    /// keeps its first position and takes the last value.
    pub fn dict(spelling: Option<Ty>, entries: Vec<(Self, Self)>) -> Self {
        let mut deduplicated: Vec<(Self, Self)> = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            match deduplicated
                .iter_mut()
                .find(|(existing, _)| *existing == key)
            {
                Some(entry) => entry.1 = value,
                None => deduplicated.push((key, value)),
            }
        }
        Self::Dict {
            spelling,
            entries: deduplicated,
        }
    }

    /// A compile-time set from display-ordered elements, first occurrence kept.
    pub fn set(spelling: Option<Ty>, elements: Vec<Self>) -> Self {
        let mut deduplicated: Vec<Self> = Vec::with_capacity(elements.len());
        for element in elements {
            if !deduplicated.contains(&element) {
                deduplicated.push(element);
            }
        }
        Self::Set {
            spelling,
            elements: deduplicated,
        }
    }

    /// Whether this value is a collection that is not implicitly copyable at
    /// runtime (`Array`/`List`, `Dict`, `Set`): it never crosses to runtime
    /// implicitly and needs an explicit `materialize[...]()`.
    pub const fn is_runtime_collection(&self) -> bool {
        matches!(self, Self::List(_) | Self::Dict { .. } | Self::Set { .. })
    }

    /// Whether this value is a runtime collection ([`Self::is_runtime_collection`])
    /// whose iteration yields values a `comptime for` binder holds: an
    /// `Int`, a `Float64`, a `Bool`, or a `String`, or a tuple or struct
    /// over such values, closed ([`Self::is_closed_aggregate`]) or holding a
    /// string an instance constructs
    /// ([`Self::is_constructed_parameter_value`]). A dictionary yields its
    /// keys.
    pub fn is_parameter_value_collection(&self) -> bool {
        let element = |value: &Self| {
            matches!(
                value,
                Self::Int(_)
                    | Self::IntLiteral(_)
                    | Self::Float(_)
                    | Self::FloatLiteral(_)
                    | Self::Bool(_)
                    | Self::Str(_)
            ) || value.is_closed_aggregate()
                || value.is_constructed_parameter_value()
        };
        match self {
            Self::List(elements) | Self::Set { elements, .. } => elements.iter().all(element),
            Self::Dict { entries, .. } => entries.iter().all(|(key, _)| element(key)),
            _ => false,
        }
    }

    /// Whether this value is a tuple or struct whose leaves are numbers and
    /// booleans: a parameter constant a backend materializes without
    /// constructing anything.
    pub fn is_closed_aggregate(&self) -> bool {
        let leaf = |value: &Self| {
            matches!(
                value,
                Self::Int(_)
                    | Self::UInt(_)
                    | Self::Index(_)
                    | Self::IntLiteral(_)
                    | Self::Float(_)
                    | Self::FloatLiteral(_)
                    | Self::Bool(_)
            ) || value.is_closed_aggregate()
        };
        match self {
            Self::Tuple(elements) => !elements.is_empty() && elements.iter().all(leaf),
            Self::Struct { fields, .. } => fields.iter().all(|(_, field)| leaf(field)),
            _ => false,
        }
    }

    /// Whether this value is one a backend materializes as a parameter
    /// constant: a vector, or a struct or tuple whose leaves are scalars,
    /// vectors, such aggregates, or frozen pointer memory.
    pub fn is_closed_parameter_value(&self) -> bool {
        match self {
            Self::Simd { .. } => true,
            Self::PopArray { elements, .. } => elements.iter().all(Self::is_closed_leaf),
            Self::Struct { fields, .. } => fields.iter().all(|(_, field)| field.is_closed_leaf()),
            Self::Tuple(elements) => elements.iter().all(Self::is_closed_leaf),
            _ => false,
        }
    }

    /// Whether this value is a closed struct or tuple: every leaf a closed
    /// parameter leaf or a string.
    pub fn is_closed_aggregate_value(&self) -> bool {
        matches!(self, Self::Struct { .. } | Self::Tuple(_))
            && (self.is_closed_parameter_value() || self.is_constructed_parameter_value())
    }

    /// Whether a bracket argument solved to this value has no run-time form
    /// at its call, which carries it as a parameter attribute alone, as
    /// Mojo's call does: a string, [compile-time-only](Self::is_compile_time_only)
    /// data, or any value [`Self::is_folded_parameter_argument`] accepts.
    pub fn is_parameter_attribute(&self) -> bool {
        matches!(self, Self::Str(_))
            || self.is_compile_time_only()
            || self.is_folded_parameter_argument()
    }

    /// Whether this value is a trait, or a parameter expression over a
    /// trait-valued binder: compile-time data with no run-time form.
    pub fn is_trait_valued(&self) -> bool {
        match self {
            Self::Trait(_) => true,
            Self::Expr(expr) => expr
                .meta()
                .as_value()
                .is_some_and(|ty| matches!(ty.kind(), TyKind::AnyTrait(_))),
            _ => false,
        }
    }

    /// Whether this value is compile-time data whose elements are types or
    /// traits, which no run-time value holds: a
    /// [trait](Self::is_trait_valued), or a parameter expression over a
    /// list of types (a type pack's `Ts.values`).
    pub fn is_metatype_valued(&self) -> bool {
        self.is_trait_valued()
            || matches!(self, Self::Expr(expr) if *expr.meta() == crate::param_expr::MetaTy::type_list())
    }

    /// Whether this value is compile-time data no run-time value holds, so a
    /// bracket argument of it is an attribute like a type: a
    /// [metatype-valued](Self::is_metatype_valued) value, a KGEN string, a
    /// raw list or pack of them or of types (`TypeList.of[Trait=AnyType,
    /// Int].values`), or a parameter expression of such a type.
    pub fn is_compile_time_only(&self) -> bool {
        self.is_metatype_valued()
            || match self {
                Self::KgenString(_)
                | Self::MlirAttr(_)
                | Self::KgenTarget(_)
                | Self::Generator(_) => true,
                Self::List(values) | Self::Tuple(values) => values
                    .iter()
                    .any(|value| matches!(value, Self::Type(_)) || value.is_compile_time_only()),
                Self::Expr(expr) => expr.meta().as_value().is_some_and(|ty| {
                    crate::types::mentions(ty, &|ty| {
                        matches!(
                            ty.kind(),
                            TyKind::KgenString
                                | TyKind::KgenDeferred
                                | TyKind::KgenTarget
                                | TyKind::Generator(_)
                        )
                    })
                }),
                _ => false,
            }
    }

    /// Whether this value is closed data a constant holds whole: a closed
    /// scalar, vector, or aggregate. A string is not one, since it owns a
    /// buffer.
    pub fn is_folded_parameter_argument(&self) -> bool {
        self.is_closed_leaf() || self.is_closed_aggregate_value()
    }

    /// Whether this value is one a backend materializes as a typed constant
    /// at its read: a [closed parameter value](Self::is_closed_parameter_value)
    /// or a struct or tuple that [owns a string
    /// buffer](Self::is_constructed_parameter_value), whose every
    /// materialization writes the frozen fields into fresh storage, or a
    /// pointer into frozen memory (a literal's bytes), which does the same.
    pub fn is_materialized_constant(&self) -> bool {
        self.is_closed_parameter_value()
            || self.is_constructed_parameter_value()
            || (matches!(self, Self::Pointer { .. }) && self.is_closed_leaf())
    }

    /// Whether this value is a struct or tuple that owns a string buffer:
    /// every leaf is a closed parameter leaf or a string, and at least one
    /// is a string. A constant of it is materialized into fresh storage at
    /// each read, and a slot holding it owns that storage.
    pub fn is_constructed_parameter_value(&self) -> bool {
        fn leaf(value: &CtValue) -> bool {
            matches!(value, CtValue::Str(_))
                || value.is_closed_leaf()
                || value.is_constructed_parameter_value()
        }
        let leaves: Vec<&Self> = match self {
            Self::Struct { fields, .. } => fields.iter().map(|(_, field)| field).collect(),
            Self::Tuple(elements) => elements.iter().collect(),
            _ => return false,
        };
        leaves.iter().all(|value| leaf(value))
            && leaves.iter().any(|value| {
                matches!(value, Self::Str(_)) || value.is_constructed_parameter_value()
            })
    }

    /// The spelling of the runtime type an un-annotated binding of this value
    /// has (upstream's wording in the materialization diagnostic: a list is
    /// `Array[T, Int(n)]`), or `None` for a compile-time-only value.
    pub fn runtime_type_text(&self) -> Option<String> {
        Some(match self {
            Self::Int(_) | Self::IntLiteral(_) => "Int".to_string(),
            Self::UInt(_) => "UInt".to_string(),
            Self::Index(_) => crate::types::MLIR_INDEX_TYPE_SPELLING.to_string(),
            Self::Float(_) | Self::FloatLiteral(_) => "Float64".to_string(),
            Self::Bool(_) => "Bool".to_string(),
            Self::None => "NoneType".to_string(),
            Self::Str(_) => "String".to_string(),
            Self::Dtype(_) => "DType".to_string(),
            Self::Simd { dtype, lanes } => {
                format!("SIMD[DType.{}, {}]", dtype.name(), lanes.len())
            }
            Self::Struct { name, .. } => name.clone(),
            Self::PopArray { .. } => self.pop_array_type()?.to_string(),
            Self::Variant { alternatives, .. } => {
                Ty::from(crate::types::TyKind::Variant(alternatives.clone())).to_string()
            }
            Self::Tuple(values) => format!(
                "Tuple[{}]",
                values
                    .iter()
                    .map(Self::runtime_type_text)
                    .collect::<Option<Vec<_>>>()?
                    .join(", ")
            ),
            Self::List(values) => format!(
                "Array[{}, Int({})]",
                values.first()?.runtime_type_text()?,
                values.len()
            ),
            Self::Dict { spelling, entries } => {
                if let Some(ty) = spelling {
                    ty.to_string()
                } else {
                    let (key, value) = entries.first()?;
                    format!(
                        "Dict[{}, {}]",
                        key.runtime_type_text()?,
                        value.runtime_type_text()?
                    )
                }
            }
            Self::Set { spelling, elements } => match spelling {
                Some(ty) => ty.to_string(),
                None => format!("Set[{}]", elements.first()?.runtime_type_text()?),
            },
            Self::Pointer { .. }
            | Self::StoreToMem(_)
            | Self::Type(_)
            | Self::Trait(_)
            | Self::KgenString(_)
            | Self::MlirAttr(_)
            | Self::KgenTarget(_)
            | Self::Generator(_)
            | Self::Expr(_)
            | Self::Deferred(_)
            | Self::Marker(_)
            | Self::RemovedArg => return None,
        })
    }

    /// The compile-time fold of a built-in scalar conversion of this value
    /// to `target`, by [`convert_lane`]; a numeric literal converts only to
    /// `Bool` (`IntLiteral.__bool__`), its numeric materialization being
    /// [`Self::materialize_as`]'s. `None` for a non-scalar, and for a float
    /// converted to `Int`/`UInt` that is not finite or whose truncation is
    /// outside the target's range: the pin's interpreter refuses that
    /// `pop.cast`, and only run time gives it the saturating value.
    pub fn folded_conversion(&self, target: ScalarConversion) -> Option<Self> {
        let lane = match self {
            Self::IntLiteral(value) if target == ScalarConversion::Bool => {
                return Some(Self::Bool(!value.is_zero()));
            }
            Self::FloatLiteral(value) if target == ScalarConversion::Bool => {
                return Some(Self::Bool(!value.is_zero()));
            }
            scalar => CtLane::of_scalar(scalar)?,
        };
        let range = match target {
            ScalarConversion::Int => Some(-(2f64.powi(63))..2f64.powi(63)),
            ScalarConversion::UInt => Some(0.0..2f64.powi(64)),
            ScalarConversion::Float64 | ScalarConversion::Bool => None,
        };
        if let (CtLane::Float(bits), Some(range)) = (&lane, range)
            && !range.contains(&f64::from_bits(*bits).trunc())
        {
            return None;
        }
        Some(convert_lane(&lane, target))
    }

    /// Materialize an exact literal into the compile-time representation of a
    /// declared scalar type. Values which are already materialized are kept as
    /// is. This is the checked boundary used by value parameters and defaults;
    /// it deliberately leaves uncontextualized literal values exact.
    ///
    /// # Panics
    ///
    /// Panics if the `List` guard above admits a target without an element
    /// type, which the type lattice forbids.
    pub fn materialize_as(self, ty: &Ty) -> Option<Self> {
        match (self, ty.kind()) {
            (value @ Self::Int(_), TyKind::INT)
            | (value @ Self::UInt(_), TyKind::UINT)
            | (value @ Self::Index(_), TyKind::Index)
            | (value @ Self::Float(_), TyKind::FLOAT64)
            | (value @ Self::IntLiteral(_), TyKind::IntLiteral)
            | (value @ Self::FloatLiteral(_), TyKind::FloatLiteral)
            | (value @ Self::Bool(_), TyKind::Bool | TyKind::I1)
            | (value @ Self::None, TyKind::None)
            | (value @ Self::Str(_), TyKind::StringLiteral(_)) => Some(value),
            // A string literal is the nominal `String`'s compile-time form
            // (`comptime E: Dict[String, Int] = {"a": 1}`).
            (value @ Self::Str(_), TyKind::Struct(name, args))
                if args.is_empty() && crate::types::is_stdlib_string_struct(name) =>
            {
                Some(value)
            }
            (value @ Self::Dtype(_), TyKind::Dtype) => Some(value),
            // A trait converts to a metatype over a trait it implies, which
            // the checker proved, and stays the trait it is.
            (value @ Self::Trait(_), TyKind::AnyTrait(_)) => Some(value),
            // A type is a value of a trait's metatype, the element of a raw
            // list of that trait whose conversion the checker proved.
            (value @ Self::Type(_), TyKind::Trait(_)) => Some(value),
            (value @ Self::KgenString(_), TyKind::KgenString) => Some(value),
            (value @ Self::MlirAttr(_), TyKind::KgenDeferred) => Some(value),
            (value @ Self::KgenTarget(_), TyKind::KgenTarget) => Some(value),
            (value @ Self::PopArray { .. }, TyKind::PopArray { .. })
                if value.pop_array_type().as_ref() == Some(ty) =>
            {
                Some(value)
            }
            // A generator binds a generator-typed parameter whose
            // conversion the checker proved.
            (value @ Self::Generator(_), TyKind::Generator(_)) => Some(value),
            // `Int`, `UInt`, and `Float64` hold their one lane as the host
            // scalar.
            (Self::Simd { dtype, lanes }, TyKind::INT | TyKind::UINT | TyKind::FLOAT64)
                if lanes.len() == 1
                    && crate::types::scalar_simd_dtype(ty.kind()) == Some(dtype) =>
            {
                match (dtype, &lanes[0]) {
                    (mojito_ast::ast::Dtype::Float64, CtLane::Float(bits)) => {
                        Some(Self::Float(*bits))
                    }
                    (mojito_ast::ast::Dtype::Int, CtLane::Int(value)) => {
                        i64::try_from(*value).ok().map(Self::Int)
                    }
                    (_, CtLane::Int(value)) => u64::try_from(*value).ok().map(Self::UInt),
                    _ => None,
                }
            }
            (
                value @ Self::Simd { .. },
                TyKind::Simd {
                    dtype: SimdDtype::Known(target),
                    width: SimdWidth::Known(width),
                },
            ) => {
                let Self::Simd { dtype, lanes } = &value else {
                    unreachable!("guard established a SIMD value");
                };
                (dtype == target && lanes.len() as i64 == *width).then_some(value)
            }
            // A frozen instance of a generic struct names its
            // backend-monomorphized instance; the target names the template.
            (value @ Self::Struct { .. }, TyKind::Struct(target, _)) => {
                let Self::Struct { name, .. } = &value else {
                    unreachable!("guard established a struct value");
                };
                (crate::types::struct_template(name) == crate::types::struct_template(target))
                    .then_some(value)
            }
            (Self::IntLiteral(value), TyKind::INT) => value.wrapping_signed(64).map(CtValue::Int),
            (Self::IntLiteral(value), TyKind::UINT) => {
                value.wrapping_unsigned(64).map(CtValue::UInt)
            }
            (Self::IntLiteral(value), TyKind::FLOAT64) => {
                value.to_f64().map(|value| Self::Float(value.to_bits()))
            }
            (Self::FloatLiteral(value), TyKind::FLOAT64) => {
                value.to_f64().map(|value| Self::Float(value.to_bits()))
            }
            (Self::IntLiteral(value), kind)
                if let Some(dtype) = crate::types::scalar_simd_dtype(kind) =>
            {
                CtLane::of_int_literal(&value, dtype).map(|lane| Self::Simd {
                    dtype,
                    lanes: vec![lane],
                })
            }
            (Self::FloatLiteral(value), kind)
                if let Some(dtype) = crate::types::scalar_simd_dtype(kind) =>
            {
                CtLane::of_float_literal(&value, dtype).map(|lane| Self::Simd {
                    dtype,
                    lanes: vec![lane],
                })
            }
            (Self::Tuple(values), TyKind::Tuple(types)) if values.len() == types.len() => values
                .into_iter()
                .zip(types)
                .map(|(value, ty)| value.materialize_as(ty))
                .collect::<Option<Vec<_>>>()
                .map(CtValue::Tuple),
            // The nominal `Tuple[...]` holds its elements at its arguments.
            (Self::Tuple(values), _) if crate::types::tuple_elements(ty).is_some() => {
                let types = crate::types::tuple_elements(ty).expect("guard established Tuple");
                (values.len() == types.len())
                    .then(|| {
                        values
                            .into_iter()
                            .zip(types)
                            .map(|(value, ty)| value.materialize_as(ty))
                            .collect::<Option<Vec<_>>>()
                            .map(CtValue::Tuple)
                    })
                    .flatten()
            }
            // A closed raw parameter list holds its elements as a tuple.
            (Self::Tuple(values), TyKind::ParamList(element)) => values
                .into_iter()
                .map(|value| value.materialize_as(element))
                .collect::<Option<Vec<_>>>()
                .map(CtValue::Tuple),
            (Self::List(values), TyKind::ComptimeList(element)) => values
                .into_iter()
                .map(|value| value.materialize_as(element))
                .collect::<Option<Vec<_>>>()
                .map(CtValue::List),
            (Self::List(values), _) if list_element(ty).is_some() => {
                let element = list_element(ty).expect("guard established List element");
                values
                    .into_iter()
                    .map(|value| value.materialize_as(element))
                    .collect::<Option<Vec<_>>>()
                    .map(CtValue::List)
            }
            (Self::Dict { entries, .. }, _) if dict_elements(ty).is_some() => {
                let (key_ty, value_ty) =
                    dict_elements(ty).expect("guard established Dict elements");
                let entries = entries
                    .into_iter()
                    .map(|(key, value)| {
                        Some((key.materialize_as(key_ty)?, value.materialize_as(value_ty)?))
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(Self::dict(Some(ty.clone()), entries))
            }
            (Self::Set { elements, .. }, _) if set_element(ty).is_some() => {
                let element_ty = set_element(ty).expect("guard established Set element");
                let elements = elements
                    .into_iter()
                    .map(|element| element.materialize_as(element_ty))
                    .collect::<Option<Vec<_>>>()?;
                Some(Self::set(Some(ty.clone()), elements))
            }
            (Self::Tuple(values), _)
                if tuple_elements(ty).is_some_and(|types| types.len() == values.len()) =>
            {
                values
                    .into_iter()
                    .zip(tuple_elements(ty).expect("guard established Tuple elements"))
                    .map(|(value, ty)| value.materialize_as(ty))
                    .collect::<Option<Vec<_>>>()
                    .map(CtValue::Tuple)
            }
            _ => None,
        }
    }

    /// Materialize this value as a literal expression, or `None` when it has no
    /// runtime form (a symbolic `Expr`, or a collection containing one).
    pub fn materialize(&self, span: Span) -> Option<Expr> {
        let kind = match self {
            Self::Int(n) => ExprKind::Int(IntLiteral::from(*n)),
            Self::UInt(n) => ExprKind::Int(IntLiteral::from(*n)),
            Self::Float(bits) => ExprKind::Float(FloatLiteral::from_f64(f64::from_bits(*bits))?),
            Self::IntLiteral(value) => ExprKind::Int(value.clone()),
            Self::FloatLiteral(value) => ExprKind::Float(value.clone()),
            Self::Bool(b) => ExprKind::Bool(*b),
            Self::None => ExprKind::None,
            Self::Str(s) => ExprKind::Str(s.clone()),
            // Upstream's spelling of an `index` constant from source:
            // `Int(n).__mlir_index__()`.
            Self::Index(n) => ExprKind::MethodCall {
                object: Box::new(literal(
                    ExprKind::Call {
                        name: "Int".to_string(),
                        param_args: Vec::new(),
                        args: vec![Self::Int(*n).materialize(span)?],
                        kwargs: Vec::new(),
                    },
                    span,
                )),
                method: "__mlir_index__".to_string(),
                args: Vec::new(),
                kwargs: Vec::new(),
            },
            Self::Tuple(vs) => ExprKind::TupleLit(materialize_all(vs, span)?),
            Self::List(vs) => ExprKind::ListLit(materialize_all(vs, span)?),
            // The bare display, or the explicit literal constructor
            // `Dict[K, V, H](keys, values, None)` when the value was spelled
            // with one (an empty explicit dict is `Dict[K, V]()`).
            Self::Dict { spelling, entries } => match spelling {
                None => ExprKind::BraceLit(
                    entries
                        .iter()
                        .map(|(key, value)| {
                            Some((key.materialize(span)?, Some(value.materialize(span)?)))
                        })
                        .collect::<Option<Vec<_>>>()?,
                ),
                Some(ty) => {
                    let Type::Named(name, param_args) = source_type(ty, span)? else {
                        return None;
                    };
                    // The key and value lists are explicit `List[K](...)`
                    // constructions, which name their element type even
                    // when a list is empty.
                    let args = if entries.is_empty() {
                        Vec::new()
                    } else {
                        let [ParamArg::Type(key_ty), ParamArg::Type(value_ty), ..] =
                            param_args.as_slice()
                        else {
                            return None;
                        };
                        let keys: Vec<&Self> = entries.iter().map(|(key, _)| key).collect();
                        let values: Vec<&Self> = entries.iter().map(|(_, value)| value).collect();
                        vec![
                            list_construction(key_ty.clone(), materialize_refs(&keys, span)?, span),
                            list_construction(
                                value_ty.clone(),
                                materialize_refs(&values, span)?,
                                span,
                            ),
                            literal(ExprKind::None, span),
                        ]
                    };
                    ExprKind::Call {
                        name,
                        param_args,
                        args,
                        kwargs: Vec::new(),
                    }
                }
            },
            Self::Set { spelling, elements } => match spelling {
                None => ExprKind::BraceLit(
                    elements
                        .iter()
                        .map(|element| Some((element.materialize(span)?, None)))
                        .collect::<Option<Vec<_>>>()?,
                ),
                Some(ty) => {
                    let Type::Named(name, param_args) = source_type(ty, span)? else {
                        return None;
                    };
                    let kwargs = if elements.is_empty() {
                        Vec::new()
                    } else {
                        vec![KwArg {
                            name: "__set_literal__".to_string(),
                            value: literal(ExprKind::None, span),
                        }]
                    };
                    ExprKind::Call {
                        name,
                        param_args,
                        args: materialize_all(elements, span)?,
                        kwargs,
                    }
                }
            },
            Self::Dtype(dtype) => ExprKind::Member {
                object: Box::new(Expr {
                    kind: ExprKind::Identifier("DType".to_string()),
                    span,
                    source: None,
                    syntax_id: mojito_common::token::SyntaxId::fresh(),
                }),
                field: dtype.name().to_string(),
            },
            // The literal's parameter read, `"text".value`, which re-folds to
            // this value.
            Self::KgenString(text) => ExprKind::Member {
                object: Box::new(Expr {
                    kind: ExprKind::Str(text.clone()),
                    span,
                    source: None,
                    syntax_id: mojito_common::token::SyntaxId::fresh(),
                }),
                field: LITERAL_VALUE_MEMBER.to_string(),
            },
            // The attribute's own spelling, `` __mlir_attr.`text` ``.
            Self::MlirAttr(text) => ExprKind::Member {
                object: Box::new(Expr {
                    kind: ExprKind::Identifier("__mlir_attr".to_string()),
                    span,
                    source: None,
                    syntax_id: mojito_common::token::SyntaxId::fresh(),
                }),
                field: text.clone(),
            },
            // The typed attribute `` __mlir_attr.`#kgen.target<…> : !kgen.target` ``.
            Self::KgenTarget(target) => ExprKind::Member {
                object: Box::new(Expr {
                    kind: ExprKind::Identifier("__mlir_attr".to_string()),
                    span,
                    source: None,
                    syntax_id: mojito_common::token::SyntaxId::fresh(),
                }),
                field: format!("{target} : !kgen.target"),
            },
            // The typed attribute `` __mlir_attr.`#pop.array<…> : !pop.array<n, index>` ``;
            // an array of any other element is frozen data a backend writes.
            Self::PopArray { element, elements }
                if matches!(element.kind(), TyKind::Index)
                    && elements.iter().all(|value| matches!(value, Self::Index(_))) =>
            {
                ExprKind::Member {
                    object: Box::new(Expr {
                        kind: ExprKind::Identifier("__mlir_attr".to_string()),
                        span,
                        source: None,
                        syntax_id: mojito_common::token::SyntaxId::fresh(),
                    }),
                    field: format!("{self} : {}", self.pop_array_type()?),
                }
            }
            // The explicit construction `SIMD[DType.d, w](l0, l1, ...)`.
            Self::Simd { dtype, lanes } => ExprKind::Call {
                name: "SIMD".to_string(),
                param_args: vec![
                    ParamArg::Value(Self::Dtype(*dtype).materialize(span)?),
                    ParamArg::Value(Self::Int(lanes.len() as i64).materialize(span)?),
                ],
                args: lanes
                    .iter()
                    .map(|lane| {
                        match lane {
                            CtLane::Int(value) => Self::IntLiteral(lane_literal(*value)),
                            CtLane::Float(bits) => Self::Float(*bits),
                            CtLane::Bool(value) => Self::Bool(*value),
                        }
                        .materialize(span)
                    })
                    .collect::<Option<Vec<_>>>()?,
                kwargs: Vec::new(),
            },
            // The fieldwise construction call; freezing guaranteed a matching
            // constructor exists.
            Self::Struct { name, fields } => ExprKind::Call {
                name: name.clone(),
                param_args: Vec::new(),
                args: fields
                    .iter()
                    .map(|(_, value)| value.materialize(span))
                    .collect::<Option<Vec<_>>>()?,
                kwargs: Vec::new(),
            },
            // Frozen memory has no source spelling: a backend writes it.
            Self::Pointer { .. }
            | Self::PopArray { .. }
            | Self::Variant { .. }
            | Self::StoreToMem(_)
            | Self::Type(_)
            | Self::Trait(_)
            | Self::Generator(_)
            | Self::Expr(_)
            | Self::Deferred(_)
            | Self::Marker(_)
            | Self::RemovedArg => return None,
        };
        Some(Expr {
            kind,
            span,
            source: None,
            syntax_id: mojito_common::token::SyntaxId::fresh(),
        })
    }

    /// A field or element of a closed aggregate parameter value: a scalar,
    /// itself a closed aggregate, or frozen memory whose present slots are
    /// closed leaves or strings (a string inside memory is the backend's to
    /// write, since no constructor builds an arbitrary pointer-owning
    /// struct).
    fn is_closed_leaf(&self) -> bool {
        match self {
            Self::Int(_)
            | Self::UInt(_)
            | Self::Index(_)
            | Self::Float(_)
            | Self::IntLiteral(_)
            | Self::FloatLiteral(_)
            | Self::Bool(_)
            | Self::Dtype(_) => true,
            // A frozen enum case: its payload, if any, is a closed leaf.
            Self::Variant { payload, .. } => payload.as_deref().is_none_or(Self::is_closed_leaf),
            Self::Pointer { memory, .. } => memory
                .iter()
                .flatten()
                .flatten()
                .all(Self::is_frozen_memory_slot),
            _ => self.is_closed_parameter_value(),
        }
    }

    /// One initialized slot of frozen memory: a closed leaf, a string, or a
    /// struct or tuple of such slots.
    fn is_frozen_memory_slot(&self) -> bool {
        match self {
            Self::Str(_) => true,
            Self::Struct { fields, .. } => fields
                .iter()
                .all(|(_, field)| field.is_frozen_memory_slot()),
            Self::Tuple(elements) => elements.iter().all(Self::is_frozen_memory_slot),
            _ => self.is_closed_leaf(),
        }
    }
}

/// The source spelling of a closed type (`Dict[String, Int, Fnv1a]`).
///
/// Scalars, nominal structs over type/value arguments, and SIMD, as a
/// materialized collection type or an initializer list's construction spells
/// it. Origin arguments and callables have no spelling here.
pub fn source_type(ty: &Ty, span: Span) -> Option<Type> {
    Some(match ty.kind() {
        TyKind::INT | TyKind::IntLiteral => Type::Int,
        TyKind::UINT => Type::UInt,
        TyKind::Bool => Type::Bool,
        TyKind::StringLiteral(_) => Type::StringLiteral,
        TyKind::FLOAT64 | TyKind::FloatLiteral => Type::Float64,
        TyKind::None => Type::None,
        TyKind::Dtype => Type::Named("DType".to_string(), Vec::new()),
        TyKind::Simd {
            dtype: SimdDtype::Known(dtype),
            width: SimdWidth::Known(width),
        } => Type::Named(
            "SIMD".to_string(),
            vec![
                ParamArg::Value(CtValue::Dtype(*dtype).materialize(span)?),
                ParamArg::Value(CtValue::Int(*width).materialize(span)?),
            ],
        ),
        // A symbolic slot has no source spelling: its declaration is still
        // a template.
        TyKind::Simd { .. } => return None,
        // A generated public-Tuple specialization keeps its element types
        // behind an argument-less symbol: spell the canonical `Tuple[...]`
        // application, which the checker maps back onto the specialization.
        TyKind::Struct(name, _)
            if name != crate::types::TUPLE_TYPE_NAME
                && let Some(elements) = crate::types::tuple_elements(ty) =>
        {
            Type::Named(
                crate::types::TUPLE_TYPE_NAME.to_string(),
                elements
                    .into_iter()
                    .map(|element| source_type(element, span).map(ParamArg::Type))
                    .collect::<Option<Vec<_>>>()?,
            )
        }
        TyKind::Struct(name, arguments) => Type::Named(
            name.clone(),
            arguments
                .iter()
                .map(|argument| match argument {
                    TyArg::Ty(ty) => Some(ParamArg::Type(source_type(ty, span)?)),
                    TyArg::Val(value) => Some(ParamArg::Value(value.materialize(span)?)),
                    // An origin tail entry has no source spelling of its own;
                    // upstream's `_` placeholder leaves the slot to inference
                    // at the materialized spelling's use.
                    TyArg::Origin(_) => Some(ParamArg::Value(Expr::new(
                        ExprKind::Identifier("_".to_string()),
                        span,
                    ))),
                })
                .collect::<Option<Vec<_>>>()?,
        ),
        _ => return None,
    })
}

/// The exact literal of one integer lane (unsigned lanes exceed `i64`).
fn lane_literal(value: i128) -> IntLiteral {
    if let Ok(value) = i64::try_from(value) {
        IntLiteral::from(value)
    } else {
        IntLiteral::from(value as u64)
    }
}

fn materialize_all(vs: &[CtValue], span: Span) -> Option<Vec<Expr>> {
    vs.iter().map(|v| v.materialize(span)).collect()
}

fn materialize_refs(vs: &[&CtValue], span: Span) -> Option<Vec<Expr>> {
    vs.iter().map(|v| v.materialize(span)).collect()
}

/// `List[T](elements..., __list_literal__=None)`.
fn list_construction(element: Type, elements: Vec<Expr>, span: Span) -> Expr {
    literal(
        ExprKind::Call {
            name: "List".to_string(),
            param_args: vec![ParamArg::Type(element)],
            args: elements,
            kwargs: vec![KwArg {
                name: "__list_literal__".to_string(),
                value: literal(ExprKind::None, span),
            }],
        },
        span,
    )
}

fn literal(kind: ExprKind, span: Span) -> Expr {
    Expr {
        kind,
        span,
        source: None,
        syntax_id: mojito_common::token::SyntaxId::fresh(),
    }
}

impl fmt::Display for CtValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(n) => write!(f, "{n}"),
            Self::UInt(n) => write!(f, "{n}u"),
            Self::Index(n) => write!(f, "{n}"),
            Self::Float(bits) => write!(f, "{:?}", f64::from_bits(*bits)),
            Self::IntLiteral(value) => write!(f, "{value}"),
            Self::FloatLiteral(value) => write!(f, "{value}"),
            Self::Bool(b) => write!(f, "{b}"),
            Self::None => write!(f, "None"),
            Self::Str(s) => write!(f, "{s:?}"),
            Self::Dtype(dtype) => write!(f, "DType.{}", dtype.name()),
            // Upstream's rendering of a SIMD parameter value.
            Self::Simd { dtype, lanes } => {
                write!(f, "[")?;
                for (index, lane) in lanes.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    match lane {
                        CtLane::Int(value) => write!(f, "{value}")?,
                        CtLane::Float(bits) => write!(f, "{:?}", f64::from_bits(*bits))?,
                        CtLane::Bool(value) => {
                            write!(f, "{}", if *value { "True" } else { "False" })?;
                        }
                    }
                }
                write!(f, "] : SIMD[DType.{}, {}]", dtype.name(), lanes.len())
            }
            Self::Struct { name, fields } => {
                write!(f, "{name}(")?;
                for (index, (_, value)) in fields.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{value}")?;
                }
                write!(f, ")")
            }
            // Frozen memory reaches instance symbols through a value
            // argument's spelling, so it is deterministic and `$`-free.
            Self::Pointer { memory, offset } => {
                write!(f, "ptr({offset}, ")?;
                match memory {
                    None => write!(f, "dangling")?,
                    Some(slots) => {
                        write!(f, "[")?;
                        for (index, slot) in slots.iter().enumerate() {
                            if index > 0 {
                                write!(f, ", ")?;
                            }
                            match slot {
                                Some(slot) => write!(f, "{slot}")?,
                                None => write!(f, "_")?,
                            }
                        }
                        write!(f, "]")?;
                    }
                }
                write!(f, ")")
            }
            Self::StoreToMem(value) => write!(f, "store_to_mem({value})"),
            Self::Variant { index, payload, .. } => match payload {
                Some(payload) => write!(f, "variant[{index}]({payload})"),
                None => write!(f, "variant[{index}]"),
            },
            Self::Type(ty) => write!(f, "{ty}"),
            Self::Trait(traits) => f.write_str(&traits.join(" & ")),
            Self::KgenString(text) => write!(f, "{text:?}"),
            Self::MlirAttr(text) => f.write_str(text),
            Self::KgenTarget(target) => write!(f, "{target}"),
            Self::PopArray { elements, .. } => {
                f.write_str("#pop.array<")?;
                for (index, element) in elements.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{element}")?;
                }
                f.write_str(">")
            }
            Self::Generator(generator) => {
                f.write_str("__gen")?;
                crate::types::write_param_decls(f, &generator.params)?;
                write!(f, " {}", generator.body)
            }
            Self::Expr(expr) => write!(f, "{expr}"),
            Self::Deferred(binder) => write!(f, "{binder}"),
            Self::Marker(marker) => write!(f, "{marker}"),
            Self::RemovedArg => f.write_str("_REMOVED_ARG"),
            Self::Dict { entries, .. } => {
                write!(f, "{{")?;
                for (index, (key, value)) in entries.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{key}: {value}")?;
                }
                write!(f, "}}")
            }
            Self::Set { elements, .. } => {
                write!(f, "{{")?;
                for (index, element) in elements.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{element}")?;
                }
                write!(f, "}}")
            }
            Self::Tuple(vs) | Self::List(vs) => {
                let (open, close) = match self {
                    Self::Tuple(_) => ('(', ')'),
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

#[cfg(test)]
mod tests {
    use super::CtLane;
    use mojito_ast::ast::Dtype;
    use mojito_common::literal::{FloatLiteral, IntLiteral};

    #[test]
    fn literal_lanes_materialize_as_the_vm_does() {
        let int = |value: i64, dtype| CtLane::of_int_literal(&IntLiteral::from(value), dtype);
        assert_eq!(int(300, Dtype::UInt8), Some(CtLane::Int(44)));
        assert_eq!(int(-1, Dtype::UInt8), Some(CtLane::Int(255)));
        assert_eq!(int(70000, Dtype::Int16), Some(CtLane::Int(4464)));
        assert_eq!(int(-2, Dtype::Int), Some(CtLane::Int(-2)));
        assert_eq!(int(1, Dtype::Bool), None);
        let tenth = FloatLiteral::parse_decimal("0.1").expect("decimal literal");
        assert_eq!(
            CtLane::of_float_literal(&tenth, Dtype::Float32),
            Some(CtLane::Float(f64::from(0.1f32).to_bits()))
        );
        assert_eq!(CtLane::of_float_literal(&tenth, Dtype::Int32), None);
        // 2^60 + 2^36 + 1 is just above the midpoint of two `float32`s; a
        // detour through `f64` lands on the midpoint and rounds to even.
        let wide = IntLiteral::from((1_i64 << 60) + (1 << 36) + 1);
        assert_eq!(
            CtLane::of_int_literal(&wide, Dtype::Float32),
            Some(CtLane::Float(
                f64::from(2f32.powi(60) + 2f32.powi(37)).to_bits()
            ))
        );
    }

    #[test]
    fn construction_lanes_convert_each_element_to_a_scalar() {
        use super::{CtValue, SimdConstructionError, simd_construction_lanes};
        let wide = CtValue::IntLiteral(IntLiteral::from((1_i64 << 60) + (1 << 36) + 1));
        assert_eq!(
            CtLane::from_value(&wide, Dtype::Float32),
            Some(CtLane::Float(
                f64::from(2f32.powi(60) + 2f32.powi(37)).to_bits()
            ))
        );
        let int32 = |lane| CtValue::Simd {
            dtype: Dtype::Int32,
            lanes: vec![CtLane::Int(lane)],
        };
        assert_eq!(
            CtLane::from_value(&int32(7), Dtype::Int32),
            Some(CtLane::Int(7))
        );
        assert_eq!(CtLane::from_value(&int32(7), Dtype::Int8), None);
        let literal = |value: i64| CtValue::IntLiteral(IntLiteral::from(value));
        assert_eq!(
            simd_construction_lanes(Dtype::UInt8, 3, &[literal(300)]),
            Ok(vec![CtLane::Int(44); 3])
        );
        assert_eq!(
            simd_construction_lanes(Dtype::Int32, 3, &[literal(1), int32(2), literal(-3)]),
            Ok(vec![CtLane::Int(1), CtLane::Int(2), CtLane::Int(-3)])
        );
        assert_eq!(
            simd_construction_lanes(Dtype::Int32, 3, &[literal(1), literal(2)]),
            Err(SimdConstructionError::Arity)
        );
        assert_eq!(
            simd_construction_lanes(Dtype::Bool, 2, &[literal(1), literal(0)]),
            Err(SimdConstructionError::Lane)
        );
        // A host `Int` is an element of an `int` vector only; a single one
        // still converts.
        let ints = [CtValue::Int(1), CtValue::Int(2)];
        assert_eq!(
            simd_construction_lanes(Dtype::UInt8, 2, &ints),
            Err(SimdConstructionError::Lane)
        );
        assert_eq!(
            simd_construction_lanes(Dtype::Int, 2, &ints),
            Ok(vec![CtLane::Int(1), CtLane::Int(2)])
        );
        assert_eq!(
            simd_construction_lanes(Dtype::UInt8, 2, &[CtValue::Int(300)]),
            Ok(vec![CtLane::Int(44); 2])
        );
    }

    #[test]
    fn lane_operators_follow_the_pop_semantics() {
        use super::{bool_lane_binop, float_lane_binop, int_lane_binop, lane_compare};
        use mojito_ast::ast::InfixOp;
        assert_eq!(int_lane_binop(InfixOp::Add, Dtype::UInt8, 250, 10), Some(4));
        assert_eq!(
            int_lane_binop(InfixOp::FloorDiv, Dtype::UInt8, 7, 0),
            Some(0)
        );
        assert_eq!(
            int_lane_binop(InfixOp::FloorDiv, Dtype::Int8, -7, 2),
            Some(-4)
        );
        assert_eq!(int_lane_binop(InfixOp::Mod, Dtype::Int8, -7, 2), Some(1));
        assert_eq!(
            int_lane_binop(InfixOp::FloorDiv, Dtype::Int8, -128, -1),
            Some(-128)
        );
        assert_eq!(int_lane_binop(InfixOp::Pow, Dtype::Int8, 2, 2), None);
        assert_eq!(
            float_lane_binop(
                InfixOp::Add,
                Dtype::Float32,
                f64::from(0.1f32),
                f64::from(0.2f32)
            ),
            Some(f64::from(0.1f32 + 0.2f32))
        );
        assert_eq!(
            float_lane_binop(InfixOp::FloorDiv, Dtype::Float32, 7.5, 2.0),
            Some(3.0)
        );
        assert_eq!(lane_compare(InfixOp::Eq, f64::NAN, f64::NAN), Some(false));
        assert_eq!(lane_compare(InfixOp::Ne, f64::NAN, f64::NAN), Some(false));
        assert_eq!(lane_compare(InfixOp::Lt, f64::NAN, f64::NAN), Some(false));
        assert_eq!(bool_lane_binop(InfixOp::Lt, false, true), Some(true));
        assert_eq!(bool_lane_binop(InfixOp::Add, false, true), None);
    }

    #[test]
    fn scalar_conversions_fold_as_the_vm_converts() {
        use super::{CtValue, ScalarConversion as To};
        let fold = |value: CtValue, to| value.folded_conversion(to);
        let float = |x: f64| CtValue::Float(x.to_bits());
        assert_eq!(
            fold(CtValue::Int(-1), To::UInt),
            Some(CtValue::UInt(u64::MAX))
        );
        assert_eq!(fold(float(3.5), To::Int), Some(CtValue::Int(3)));
        assert_eq!(fold(float(-0.5), To::UInt), Some(CtValue::UInt(0)));
        assert_eq!(fold(float(1e30), To::Int), None);
        assert_eq!(fold(float(f64::NAN), To::Int), None);
        assert_eq!(fold(float(-1.0), To::UInt), None);
        assert_eq!(fold(float(f64::NAN), To::Bool), Some(CtValue::Bool(false)));
        let lane = |dtype, lane| CtValue::Simd {
            dtype,
            lanes: vec![lane],
        };
        assert_eq!(
            fold(lane(Dtype::UInt8, CtLane::Int(200)), To::Int),
            Some(CtValue::Int(200))
        );
        let tenth = f64::from(0.1f32);
        assert_eq!(
            fold(
                lane(Dtype::Float32, CtLane::Float(tenth.to_bits())),
                To::Float64
            ),
            Some(float(tenth))
        );
        assert_eq!(
            fold(CtValue::IntLiteral(IntLiteral::from(1)), To::Bool),
            Some(CtValue::Bool(true))
        );
        assert_eq!(
            fold(CtValue::IntLiteral(IntLiteral::from(7)), To::Int),
            None
        );
    }
}

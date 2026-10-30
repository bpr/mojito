//! Runtime values and their operations, used by the register-VM `backend`.
//!
//! This module owns the `Value` type (`SimdLanes` and all), scalar/`SIMD`
//! operations, coercion, and the utility built-ins — the shared value layer
//! the backend consumes.

use std::cmp::Ordering;
use std::fmt;
use std::io::{self, Write};

use mojito_ast::ast::{Dtype, InfixOp, PrefixOp, Type};
use mojito_types::ct::{
    CtLane, CtValue, ScalarConversion, bool_lane_binop, convert_lane, float_lane_binop,
    int_lane_binop, lane_compare, wrap_lane,
};
use mojito_types::types::{Ty, canonical_simd_ty};

/// A runtime value produced by evaluating an expression.
#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    UInt(u64),
    Float64(f64),
    /// Arbitrary-precision compile-time integer, materialized before it can
    /// inhabit an ordinary runtime binding.
    IntLiteral(mojito_common::literal::IntLiteral),
    /// Exact finite compile-time float, materialized once into a runtime scalar.
    FloatLiteral(mojito_common::literal::FloatLiteral),
    Bool(bool),
    Str(String),
    None,
    /// A non-capturing function value, represented by its resolved MIR symbol.
    Function(String),
    /// A lifted non-escaping function plus its explicit capture environment.
    Closure {
        function: String,
        captures: Vec<ClosureCapture>,
    },
    /// A struct instance: its type name and field values (in declaration
    /// order). An instance's compile-time arguments are in its name, as the
    /// elaborator minted it. A value type — `Clone` deep-copies, giving value
    /// semantics.
    Struct {
        name: String,
        fields: Vec<(String, Self)>,
    },
    /// A SIMD vector: its element type and lane values. The width-1 case is a
    /// scalar-alias value (`Int32`, …).
    Simd {
        dtype: Dtype,
        lanes: SimdLanes,
    },
    /// A `DType` value.
    Dtype(Dtype),
    /// An attribute held as a value, as upstream's `IREvaluator` holds
    /// every value as an `Attribute`: it is copied, moved, passed,
    /// returned, and frozen back as itself. An `!kgen.deferred` attribute
    /// is inspected by no instruction, and only an elaborator request runs
    /// a body holding one, since translation rejects one that survives the
    /// pipeline. A `!kgen.string` is also a run-time value, which the
    /// `pop.string.size` and `pop.string.address` reads inspect.
    Attribute(mojito_types::ct::CtValue),
    /// An `Error` value carrying its message (what `raise` raises).
    Error(String),
    /// A list-shaped compile-time value crossing the narrow VM-CTFE bridge.
    /// This is never a language-level runtime collection: executable List values
    /// are ordinary nominal [`Value::Struct`] instances from the bundled library.
    ComptimeList(Vec<Self>),
    /// Internal heterogeneous pack storage shared by MIR and VM-CTFE. Public
    /// `Tuple` values are nominal structs and must not use this representation.
    Tuple(Vec<Self>),
    /// A tagged union. `alternatives` is the checked type-level ordering and
    /// `index` selects the active payload.
    Variant {
        alternatives: Vec<mojito_types::types::Ty>,
        index: usize,
        value: Box<Self>,
    },
    /// An `UnsafePointer[T]`: stable allocation identity plus a typed element
    /// offset. Allocation zero is reserved for non-null dangling placeholders.
    Pointer {
        allocation: u64,
        offset: i64,
    },
    /// A verified reference to a variable slot in a live VM frame. Projection
    /// indexes are captured when the handle is created, so later evaluation
    /// cannot silently retarget the reference.
    Ref {
        frame: u64,
        slot: usize,
        projection: Vec<RefProjection>,
    },
    /// A **tombstone** left when a value is moved out of a variable slot (`b = a^`)
    /// in the VM backend: the source slot holds `Moved` afterward, so a
    /// use-after-move surfaces as a loud runtime error (a defensive check — the
    /// ownership analysis already rejects it statically), and a no-op to drop.
    Moved,
    /// Inline possibly-uninitialized storage (`MaybeUninit`'s field):
    /// `None` is uninitialized, `Some` holds the raw payload. Dropping one is
    /// a no-op regardless of contents — the payload leaks by design, matching
    /// upstream's no-op destructor — and reading/taking the `None` state traps.
    /// `None` is also the value of [`Const::UninitMem`](mojito_mir::mir::Const::UninitMem),
    /// uninitialized memory that is copied, stored, and dropped freely.
    UninitStorage(Option<Box<Self>>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefProjection {
    Field(String),
    Index(usize),
    /// The offset-0 dereference of a single-pointee `to=place` pointer
    /// (`p[]`): the handle stored at this step already designates the
    /// pointee, so the segment is the identity — distinct from `Index(0)`,
    /// which selects element 0 of a collection reached the same way.
    Deref,
    Variant(usize),
    /// One owned slot in a closure's declaration-created environment.
    Capture(usize),
    /// The payload of a [`Value::UninitStorage`] slot; reading it while the
    /// storage is uninitialized traps.
    UninitPayload,
}

/// One slot in a lifted closure's environment.
///
/// Reference captures retain their original frame handle; a moved capture,
/// a lambda's storage, is owned by the closure and borrowed in place on
/// every invocation.
#[derive(Debug, Clone)]
pub struct ClosureCapture {
    pub value: Value,
    pub owned: bool,
    /// The `__deinit__` an owned capture's value runs when the closure
    /// value dies, as the `closure.make` capture names it; `None` for a
    /// reference, or for a value whose type names none.
    pub destructor: Option<String>,
}

/// The lanes of a SIMD value, one representation per element-type kind.
///
/// Integer lanes are stored as `i128` holding the post-wrap mathematical value
/// (so an unsigned lane is its non-negative value), which makes comparisons
/// correct for both signed and unsigned dtypes.
#[derive(Debug, Clone, PartialEq)]
pub enum SimdLanes {
    Int(Vec<i128>),
    Float(Vec<f64>),
    Bool(Vec<bool>),
}

impl SimdLanes {
    pub const fn width(&self) -> usize {
        match self {
            Self::Int(v) => v.len(),
            Self::Float(v) => v.len(),
            Self::Bool(v) => v.len(),
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Int(a), Self::Int(b)) => a == b,
            (Self::UInt(a), Self::UInt(b)) => a == b,
            (Self::Float64(a), Self::Float64(b)) => a == b,
            (Self::IntLiteral(a), Self::IntLiteral(b)) => a == b,
            (Self::FloatLiteral(a), Self::FloatLiteral(b)) => a == b,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Dtype(a), Self::Dtype(b)) => a == b,
            (Self::Attribute(a), Self::Attribute(b)) => a == b,
            (Self::Str(a), Self::Str(b)) => a == b,
            (
                Self::Pointer {
                    allocation: aa,
                    offset: ao,
                },
                Self::Pointer {
                    allocation: ba,
                    offset: bo,
                },
            ) => aa == ba && ao == bo,
            (Self::None, Self::None) => true,
            // Closures have identity, not structural, equality.
            (
                Self::Struct {
                    name: n1,
                    fields: f1,
                },
                Self::Struct {
                    name: n2,
                    fields: f2,
                },
            ) => n1 == n2 && f1 == f2,
            (
                Self::Simd {
                    dtype: d1,
                    lanes: l1,
                },
                Self::Simd {
                    dtype: d2,
                    lanes: l2,
                },
            ) => d1 == d2 && l1 == l2,
            (Self::Error(a), Self::Error(b)) => a == b,
            (Self::ComptimeList(a), Self::ComptimeList(b)) => a == b,
            (Self::Tuple(a), Self::Tuple(b)) => a == b,
            (
                Self::Variant {
                    alternatives: aa,
                    index: ai,
                    value: av,
                },
                Self::Variant {
                    alternatives: ba,
                    index: bi,
                    value: bv,
                },
            ) => aa == ba && ai == bi && av == bv,
            (
                Self::Ref {
                    frame: af,
                    slot: as_,
                    projection: ap,
                },
                Self::Ref {
                    frame: bf,
                    slot: bs,
                    projection: bp,
                },
            ) => af == bf && as_ == bs && ap == bp,
            (Self::UninitStorage(a), Self::UninitStorage(b)) => a == b,
            _ => false,
        }
    }
}

/// The display text of a `Float64`: Rust's shortest round-trip `{:?}` (which
/// keeps a decimal point or exponent and spells infinities `inf`/`-inf`),
/// with NaN spelled `nan` as Mojo prints it.
pub fn float_display(value: f64) -> String {
    if value.is_nan() {
        "nan".to_string()
    } else {
        format!("{value:?}")
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(n) => write!(f, "{n}"),
            Self::UInt(n) => write!(f, "{n}"),
            // `{:?}` keeps the decimal point (e.g. `3.0`), distinguishing from
            // Int; NaN spells `nan` as upstream prints it.
            Self::Float64(x) => write!(f, "{}", float_display(*x)),
            Self::IntLiteral(value) => write!(f, "{value}"),
            Self::FloatLiteral(value) => write!(f, "{value}"),
            Self::Bool(b) => write!(f, "{}", if *b { "True" } else { "False" }),
            Self::Dtype(dtype) => write!(f, "{}", dtype.name()),
            Self::Str(s) => write!(f, "{s}"),
            Self::None => write!(f, "None"),
            Self::Function(name) => write!(f, "<function {name}>"),
            Self::Closure { function, .. } => write!(f, "<closure {function}>"),
            Self::Struct { name, fields } => {
                write!(f, "{name}(")?;
                for (i, (fname, val)) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{fname}={val}")?;
                }
                write!(f, ")")
            }
            // A width-1 SIMD prints as the bare lane; wider as `[l0, l1, ...]`.
            Self::Simd { lanes, .. } => {
                let strs: Vec<String> = match lanes {
                    SimdLanes::Int(v) => v.iter().map(std::string::ToString::to_string).collect(),
                    SimdLanes::Float(v) => v.iter().map(|x| float_display(*x)).collect(),
                    SimdLanes::Bool(v) => v
                        .iter()
                        .map(|b| {
                            if *b {
                                "True".to_string()
                            } else {
                                "False".to_string()
                            }
                        })
                        .collect(),
                };
                if strs.len() == 1 {
                    write!(f, "{}", strs[0])
                } else {
                    write!(f, "[{}]", strs.join(", "))
                }
            }
            // Upstream prints an Error as its bare message (`print(e)` ->
            // `boom`), not a constructor rendering.
            Self::Error(msg) => write!(f, "{msg}"),
            Self::Pointer { allocation, offset } => {
                write!(f, "Pointer({allocation:#x}+{offset})")
            }
            Self::Ref { frame, slot, .. } => write!(f, "<ref {frame}:{slot}>"),
            Self::Moved => write!(f, "<moved>"),
            Self::UninitStorage(None) => write!(f, "<uninit>"),
            Self::UninitStorage(Some(value)) => write!(f, "<uninit-storage {value}>"),
            Self::ComptimeList(items) => {
                write!(f, "<comptime-list [")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{item}")?;
                }
                write!(f, "]>")
            }
            Self::Tuple(items) => {
                write!(f, "(")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{item}")?;
                }
                // A 1-tuple prints with a trailing comma, like Python/Mojo.
                if items.len() == 1 {
                    write!(f, ",")?;
                }
                write!(f, ")")
            }
            Self::Variant { value, .. } => value.fmt(f),
            Self::Attribute(value) => write!(f, "{value}"),
        }
    }
}

/// The Mojo type name of a runtime value, for error messages.
pub fn type_name(value: &Value) -> String {
    match value {
        Value::Int(_) => "Int".to_string(),
        Value::UInt(_) => "UInt".to_string(),
        Value::Float64(_) => "Float64".to_string(),
        Value::IntLiteral(_) => "IntLiteral".to_string(),
        Value::FloatLiteral(_) => "FloatLiteral".to_string(),
        Value::Bool(_) => "Bool".to_string(),
        Value::Dtype(_) => "DType".to_string(),
        Value::Attribute(mojito_types::ct::CtValue::KgenString(_)) => {
            mojito_types::types::KGEN_STRING_TYPE_SPELLING.to_string()
        }
        Value::Attribute(_) => mojito_types::types::KGEN_DEFERRED_TYPE_SPELLING.to_string(),
        Value::Str(_) => "String".to_string(),
        Value::None => "None".to_string(),
        Value::Function(_) | Value::Closure { .. } => "function".to_string(),
        Value::Struct { name, .. } => name.clone(),
        Value::Simd { dtype, lanes } => {
            if lanes.width() == 1 {
                dtype.scalar_alias().unwrap_or("SIMD").to_string()
            } else {
                format!("SIMD[DType.{}, {}]", dtype.name(), lanes.width())
            }
        }
        Value::Error(_) => "Error".to_string(),
        Value::Pointer { .. } => "Pointer".to_string(),
        Value::Ref { .. } => "ref".to_string(),
        Value::Moved => "<moved>".to_string(),
        Value::UninitStorage(_) => "__UninitStorage".to_string(),
        Value::ComptimeList(_) => "<comptime-list>".to_string(),
        Value::Tuple(items) => {
            let elems: Vec<String> = items.iter().map(type_name).collect();
            format!("Tuple[{}]", elems.join(", "))
        }
        Value::Variant { alternatives, .. } => {
            let names: Vec<String> = alternatives.iter().map(ToString::to_string).collect();
            format!("Variant[{}]", names.join(", "))
        }
    }
}

/// Structural equality for `==`/`!=`. Numeric values use ordinary promotion;
/// tuples recurse element-wise. Other scalar values must have the same type.
#[allow(
    clippy::float_cmp,
    reason = "exact comparison is the semantics of Mojo `==` on Float64"
)]
pub fn values_equal(a: &Value, b: &Value) -> Result<bool, RuntimeError> {
    if let (Some(a), Some(b)) = (as_finite_numeric(a), as_finite_numeric(b)) {
        return Ok(a.compare(&b) == Ordering::Equal);
    }
    // Exact literals are always finite. If the other numeric value could not
    // enter the exact domain, it is an infinity or NaN and therefore unequal.
    if (as_exact_num(a).is_some() && as_num(b).is_some())
        || (as_num(a).is_some() && as_exact_num(b).is_some())
    {
        return Ok(false);
    }
    if let (Some(a), Some(b)) = (as_num(a), as_num(b)) {
        return Ok(match a.rank().max(b.rank()) {
            2 => a.as_f64() == b.as_f64(),
            1 => a.as_u64() == b.as_u64(),
            _ => a.as_i64() == b.as_i64(),
        });
    }
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Ok(x == y),
        (Value::UInt(x), Value::UInt(y)) => Ok(x == y),
        (Value::Float64(x), Value::Float64(y)) => Ok(x == y),
        (Value::Bool(x), Value::Bool(y)) => Ok(x == y),
        (Value::Dtype(x), Value::Dtype(y)) => Ok(x == y),
        (Value::Str(x), Value::Str(y)) => Ok(x == y),
        (Value::None, Value::None) => Ok(true),
        (
            Value::Variant {
                alternatives: aa,
                index: ai,
                value: av,
            },
            Value::Variant {
                alternatives: ba,
                index: bi,
                value: bv,
            },
        ) if aa == ba => {
            if ai == bi {
                values_equal(av, bv)
            } else {
                Ok(false)
            }
        }
        (Value::Tuple(left), Value::Tuple(right)) => {
            if left.len() != right.len() {
                return Ok(false);
            }
            for (left, right) in left.iter().zip(right) {
                match values_equal(left, right) {
                    Ok(true) => {}
                    Ok(false) | Err(_) => return Ok(false),
                }
            }
            Ok(true)
        }
        (Value::ComptimeList(left), Value::ComptimeList(right)) => {
            if left.len() != right.len() {
                return Ok(false);
            }
            for (left, right) in left.iter().zip(right) {
                if !values_equal(left, right).unwrap_or(false) {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        _ => Err(RuntimeError::TypeError(format!(
            "cannot compare {} and {}",
            type_name(a),
            type_name(b)
        ))),
    }
}

/// The checked type a runtime `Bool` or `DType` Hashable leaf hashes as.
///
/// The hasher's `_update_with_simd` instance is selected by it. A `SIMD`
/// hashes through its declared `__hash__`, so only these two leaves remain.
pub fn hash_leaf_ty(value: &Value) -> Option<Ty> {
    Some(match value {
        Value::Bool(_) => Ty::bool(),
        // Upstream's `DType.__hash__` hashes its code as a `UInt8`.
        Value::Dtype(_) => canonical_simd_ty(Dtype::UInt8, 1),
        _ => return None,
    })
}

/// The value a Hashable leaf contributes to its hasher, typed as `hash_leaf_ty`
/// names it: a `DType` contributes its code, and a `Bool` itself.
pub fn hash_leaf_value(value: Value) -> Value {
    match value {
        Value::Dtype(dtype) => Value::Simd {
            dtype: Dtype::UInt8,
            lanes: SimdLanes::Int(vec![i128::from(dtype.code())]),
        },
        other => other,
    }
}

/// Reinterpret `value`'s bits as `width` lanes of `target`, upstream's
/// `pop.bitcast`.
///
/// The lanes pack little-endian, lane 0 lowest, and a `bool` lane is one bit.
/// The total bit width is kept, which the checker and the MIR verifier
/// guarantee.
pub fn simd_bitcast(target: Dtype, width: usize, value: &Value) -> Result<Value, RuntimeError> {
    let (dtype, lanes) = match value {
        Value::Bool(b) => (Dtype::Bool, SimdLanes::Bool(vec![*b])),
        other => simd_parts(other).ok_or_else(|| {
            RuntimeError::TypeError(format!("cannot bitcast {}", type_name(other)))
        })?,
    };
    let source_bits = dtype.bit_width() as usize;
    let target_bits = target.bit_width() as usize;
    let raw: Vec<u64> = match &lanes {
        SimdLanes::Int(values) => values.iter().map(|lane| *lane as u64).collect(),
        SimdLanes::Float(values) => values
            .iter()
            .map(|lane| match dtype {
                Dtype::Float16 => u64::from(mojito_common::float::f16_bits(*lane)),
                Dtype::Float32 => u64::from((*lane as f32).to_bits()),
                _ => lane.to_bits(),
            })
            .collect(),
        SimdLanes::Bool(values) => values.iter().map(|lane| u64::from(*lane)).collect(),
    };
    if raw.len() * source_bits != width * target_bits {
        return Err(RuntimeError::TypeError(format!(
            "cannot bitcast {} {} lane(s) to {width} {} lane(s)",
            raw.len(),
            dtype.name(),
            target.name()
        )));
    }
    let mut words = vec![0_u64; (width * target_bits).div_ceil(64)];
    for (index, lane) in raw.into_iter().enumerate() {
        let offset = index * source_bits;
        words[offset / 64] |= (lane & low_bits(source_bits)) << (offset % 64);
    }
    let read = |index: usize| {
        let offset = index * target_bits;
        (words[offset / 64] >> (offset % 64)) & low_bits(target_bits)
    };
    let lanes = match target {
        Dtype::Bool => SimdLanes::Bool((0..width).map(|index| read(index) != 0).collect()),
        Dtype::Float16 => SimdLanes::Float(
            (0..width)
                .map(|index| mojito_common::float::f16_from_bits(read(index) as u16))
                .collect(),
        ),
        Dtype::Float32 => SimdLanes::Float(
            (0..width)
                .map(|index| f64::from(f32::from_bits(read(index) as u32)))
                .collect(),
        ),
        Dtype::Float64 => SimdLanes::Float(
            (0..width)
                .map(|index| f64::from_bits(read(index)))
                .collect(),
        ),
        _ => {
            let shift = 64 - target_bits as u32;
            let signed = integer_dtype_bits(target).is_some_and(|(_, signed)| signed);
            SimdLanes::Int(
                (0..width)
                    .map(|index| {
                        let raw = read(index);
                        if signed {
                            i128::from(((raw << shift) as i64) >> shift)
                        } else {
                            i128::from(raw)
                        }
                    })
                    .collect(),
            )
        }
    };
    Ok(simd_value(target, lanes))
}

/// Apply a unary operator to an already-evaluated operand for VM execution and
/// VM-backed compile-time evaluation.
pub fn apply_prefix(op: PrefixOp, value: Value) -> Result<Value, RuntimeError> {
    match (op, value) {
        (PrefixOp::Neg, Value::Int(n)) => Ok(Value::Int(n.wrapping_neg())),
        (PrefixOp::Neg, Value::Float64(x)) => Ok(Value::Float64(-x)),
        (PrefixOp::Neg, Value::IntLiteral(value)) => Ok(Value::IntLiteral(value.neg())),
        (PrefixOp::Neg, Value::FloatLiteral(value)) => Ok(Value::FloatLiteral(value.neg())),
        (PrefixOp::Pos, value @ (Value::IntLiteral(_) | Value::FloatLiteral(_))) => Ok(value),
        (PrefixOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
        // Bitwise inversion: two's complement on integers, logical on `Bool`.
        (PrefixOp::Invert, Value::Int(n)) => Ok(Value::Int(!n)),
        (PrefixOp::Invert, Value::UInt(n)) => Ok(Value::UInt(!n)),
        (PrefixOp::Invert, Value::Bool(b)) => Ok(Value::Bool(!b)),
        (PrefixOp::Invert, Value::IntLiteral(value)) => Ok(Value::IntLiteral(
            value
                .neg()
                .sub(&mojito_common::literal::IntLiteral::from(1i64)),
        )),
        (PrefixOp::Invert, Value::Simd { dtype, lanes }) => match lanes {
            SimdLanes::Int(v) => Ok(simd_value(
                dtype,
                SimdLanes::Int(v.into_iter().map(|x| wrap_lane(dtype, !x)).collect()),
            )),
            SimdLanes::Bool(v) => Ok(simd_value(
                dtype,
                SimdLanes::Bool(v.into_iter().map(|x| !x).collect()),
            )),
            SimdLanes::Float(_) => Err(RuntimeError::TypeError(
                "cannot invert a float SIMD value".to_string(),
            )),
        },
        // Elementwise negation wraps integer lanes at the element width;
        // float lanes negate exactly (a sign flip never re-rounds).
        (PrefixOp::Neg, Value::Simd { dtype, lanes }) => match lanes {
            SimdLanes::Int(v) => Ok(simd_value(
                dtype,
                SimdLanes::Int(
                    v.into_iter()
                        .map(|x| wrap_lane(dtype, x.wrapping_neg()))
                        .collect(),
                ),
            )),
            SimdLanes::Float(v) => Ok(simd_value(
                dtype,
                SimdLanes::Float(v.into_iter().map(|x| -x).collect()),
            )),
            SimdLanes::Bool(_) => Err(RuntimeError::TypeError(
                "cannot negate a bool SIMD mask".to_string(),
            )),
        },
        (PrefixOp::Pos, v) => Err(RuntimeError::TypeError(format!(
            "unary '+' over {} is its `__pos__` call",
            type_name(&v)
        ))),
        (PrefixOp::Neg, v) => Err(RuntimeError::TypeError(format!(
            "cannot negate {}",
            type_name(&v)
        ))),
        (PrefixOp::Not, v) => Err(RuntimeError::TypeError(format!(
            "'not' requires Bool, got {}",
            type_name(&v)
        ))),
        (PrefixOp::Invert, v) => Err(RuntimeError::TypeError(format!(
            "cannot invert {}",
            type_name(&v)
        ))),
    }
}

/// core of `eval_infix` and augmented assignment. `and`/`or` short-circuit and so
/// are handled by the caller, not here.
pub fn apply_infix(op: InfixOp, l: Value, r: Value) -> Result<Value, RuntimeError> {
    // Membership `in` / `not in`.
    if matches!(op, InfixOp::In | InfixOp::NotIn) {
        return eval_membership(op, &l, &r);
    }
    // Elementwise SIMD operators (before the scalar-numeric path).
    if matches!(l, Value::Simd { .. }) || matches!(r, Value::Simd { .. }) {
        return simd_binop(op, &l, &r);
    }
    if let (Some(left), Some(right)) = (as_exact_num(&l), as_exact_num(&r)) {
        return exact_numeric_op(op, left, right);
    }
    if as_exact_num(&l).is_some() && as_num(&r).is_some() {
        return apply_infix(op, materialize_like_numeric(l, &r)?, r);
    }
    if as_num(&l).is_some() && as_exact_num(&r).is_some() {
        return apply_infix(op, l.clone(), materialize_like_numeric(r, &l)?);
    }
    // `+` on two strings concatenates.
    if let (InfixOp::Add, Value::Str(a), Value::Str(b)) = (op, &l, &r) {
        return Ok(Value::Str(format!("{a}{b}")));
    }
    // Numeric operators (arithmetic, ordering, and numeric equality), with
    // promotion of mixed numeric kinds.
    if let (Some(a), Some(b)) = (as_num(&l), as_num(&r)) {
        return numeric_op(op, a, b);
    }
    // Equality between non-numeric values (Bool/String/None), and `Bool`'s
    // `Comparable` order, `False` before `True`.
    match (op, &l, &r) {
        (InfixOp::Eq, ..) => Ok(Value::Bool(values_equal(&l, &r)?)),
        (InfixOp::Ne, ..) => Ok(Value::Bool(!values_equal(&l, &r)?)),
        (InfixOp::Lt, Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a < b)),
        (InfixOp::Le, Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a <= b)),
        (InfixOp::Gt, Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a > b)),
        (InfixOp::Ge, Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a >= b)),
        (InfixOp::BitAnd, Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a & b)),
        (InfixOp::BitOr, Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a | b)),
        (InfixOp::BitXor, Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a ^ b)),
        _ => Err(RuntimeError::TypeError(format!(
            "operator {:?} is not defined for {} and {}",
            op,
            type_name(&l),
            type_name(&r)
        ))),
    }
}

/// A `Value` used as an index, requiring it to be an `Int`.
pub fn value_as_index(v: &Value) -> Result<i64, RuntimeError> {
    match v {
        Value::Int(n) => Ok(*n),
        Value::IntLiteral(value) => value
            .to_i64()
            .ok_or_else(|| literal_materialization_error(value, "Int index")),
        other => Err(RuntimeError::TypeError(format!(
            "index must be Int, got {}",
            type_name(other)
        ))),
    }
}

/// Evaluate primitive/internal `x in c` / `x not in c`.
///
/// Nominal collections dispatch through `__contains__`; this helper handles
/// only internal pack storage, the compile-time-only list bridge, and
/// `String`.
pub fn eval_membership(op: InfixOp, l: &Value, r: &Value) -> Result<Value, RuntimeError> {
    let found = match r {
        Value::ComptimeList(items) => {
            let target = match items.first() {
                Some(f) => coerce_like(l.clone(), f),
                None => l.clone(),
            };
            items
                .iter()
                .any(|it| values_equal(it, &target).unwrap_or(false))
        }
        Value::Tuple(items) => items
            .iter()
            .any(|item| values_equal(item, l).unwrap_or(false)),
        Value::Str(s) => match l {
            Value::Str(sub) => s.contains(sub.as_str()),
            other => {
                return Err(RuntimeError::TypeError(format!(
                    "left operand of 'in' on a String must be a String, got {}",
                    type_name(other)
                )));
            }
        },
        other => {
            return Err(RuntimeError::TypeError(format!(
                "'in' requires a nominal collection, internal Tuple, or String, got {}",
                type_name(other)
            )));
        }
    };
    let result = if op == InfixOp::NotIn { !found } else { found };
    Ok(Value::Bool(result))
}

/// `min(a, b)` / `max(a, b)`: promote the operands to a common numeric kind (as
/// arithmetic does) and return the smaller/larger, rebuilt in that kind.
pub fn builtin_min_max(is_min: bool, a: Value, b: Value) -> Result<Value, RuntimeError> {
    if let (Some(left), Some(right)) = (as_exact_num(&a), as_exact_num(&b)) {
        let left_first = (left.compare(&right) != Ordering::Greater) == is_min;
        return Ok(if left_first { a } else { b });
    }
    if as_exact_num(&a).is_some() && as_num(&b).is_some() {
        return builtin_min_max(is_min, materialize_like_numeric(a, &b)?, b);
    }
    if as_num(&a).is_some() && as_exact_num(&b).is_some() {
        return builtin_min_max(is_min, a.clone(), materialize_like_numeric(b, &a)?);
    }
    let (Some(na), Some(nb)) = (as_num(&a), as_num(&b)) else {
        return Err(RuntimeError::TypeError(format!(
            "min()/max() expect numeric values, got {} and {}",
            type_name(&a),
            type_name(&b)
        )));
    };
    Ok(match na.rank().max(nb.rank()) {
        2 => {
            let (x, y) = (na.as_f64(), nb.as_f64());
            Value::Float64(if (x <= y) == is_min { x } else { y })
        }
        1 => {
            let (x, y) = (na.as_u64(), nb.as_u64());
            Value::UInt(if (x <= y) == is_min { x } else { y })
        }
        _ => {
            let (x, y) = (na.as_i64(), nb.as_i64());
            Value::Int(if (x <= y) == is_min { x } else { y })
        }
    })
}

/// `round(x)`: nearest `Float64`, ties to even, as upstream's `round` and
/// `llvm.roundeven.f64` do. An integer is already whole, so it is returned
/// unchanged (`__round__(self) -> Self`).
pub fn builtin_round(v: &Value) -> Result<Value, RuntimeError> {
    match v {
        Value::Int(_) | Value::UInt(_) => Ok(v.clone()),
        _ => Ok(Value::Float64(value_to_float(v)?.round_ties_even())),
    }
}

/// `input(prompt)`: write the prompt immediately, read one line from standard
/// input, and return it without the trailing line ending.
///
/// End of input raises `Error("EOF")`, as upstream's raising `input` does.
pub fn builtin_input(prompt: Value) -> Result<Value, RuntimeError> {
    let Value::Str(prompt) = prompt else {
        return Err(RuntimeError::TypeError(format!(
            "input() expects a String prompt, got {}",
            type_name(&prompt)
        )));
    };

    print!("{prompt}");
    io::stdout()
        .flush()
        .map_err(|e| RuntimeError::Unsupported(format!("input(): failed to flush stdout: {e}")))?;

    let mut line = String::new();
    let read = io::stdin()
        .read_line(&mut line)
        .map_err(|e| RuntimeError::Unsupported(format!("input(): failed to read stdin: {e}")))?;
    if read == 0 {
        return Err(input_eof());
    }
    if line.ends_with('\n') {
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
    }
    Ok(Value::Str(line))
}

/// The error `input()` raises when standard input is exhausted.
pub fn input_eof() -> RuntimeError {
    RuntimeError::Raised(Value::Error("EOF".to_string()))
}

/// The synthetic integer address of a VM pointer: 0 for the null (allocation
/// 0) pointer, otherwise the allocation id in the high word with the element
/// offset added, so distinct provenances never collide.
pub const fn pointer_address(allocation: u64, offset: i64) -> i64 {
    if allocation == 0 {
        return 0;
    }
    ((allocation as i64) << 32).wrapping_add(offset)
}

/// The `scalar.cast` to `target`: `Bool(x)`, a literal's conversion, and
/// the `__bool__`, `__int__`, and `__float__` of a `Bool` or a literal
/// receiver.
///
/// Convert one numeric-or-`Bool` value (`Float64`→integer truncates toward
/// zero, `Bool` is 0/1, `Bool(x)` is truthiness — a nonzero value is `True`),
/// following Mojo.
pub fn builtin_convert(target: ScalarConversion, v: Value) -> Result<Value, RuntimeError> {
    match &v {
        Value::IntLiteral(value) => {
            return Ok(match target {
                ScalarConversion::Int => Value::Int(
                    value
                        .wrapping_signed(64)
                        .ok_or_else(|| literal_materialization_error(&value, "Int"))?,
                ),
                ScalarConversion::UInt => Value::UInt(
                    value
                        .wrapping_unsigned(64)
                        .ok_or_else(|| literal_materialization_error(&value, "UInt"))?,
                ),
                ScalarConversion::Bool => Value::Bool(!value.is_zero()),
                ScalarConversion::Float64 => Value::Float64(
                    value
                        .to_f64()
                        .ok_or_else(|| literal_materialization_error(&value, "Float64"))?,
                ),
            });
        }
        Value::FloatLiteral(value) => {
            return Ok(match target {
                ScalarConversion::Int => Value::Int(
                    value
                        .trunc_to_int()
                        .wrapping_signed(64)
                        .ok_or_else(|| literal_materialization_error(&value, "Int"))?,
                ),
                ScalarConversion::UInt => Value::UInt(
                    value
                        .trunc_to_int()
                        .wrapping_unsigned(64)
                        .ok_or_else(|| literal_materialization_error(&value, "UInt"))?,
                ),
                ScalarConversion::Bool => Value::Bool(!value.is_zero()),
                ScalarConversion::Float64 => Value::Float64(
                    value
                        .to_f64()
                        .ok_or_else(|| literal_materialization_error(&value, "Float64"))?,
                ),
            });
        }
        _ => {}
    }
    let lane = match v {
        Value::Int(n) => CtLane::Int(i128::from(n)),
        Value::UInt(n) => CtLane::Int(i128::from(n)),
        Value::Float64(x) => CtLane::Float(x.to_bits()),
        Value::Bool(b) => CtLane::Bool(b),
        // A width-1 SIMD value is a scalar alias (`UInt8`, `Byte`, ...).
        Value::Simd { ref lanes, .. } if lanes.width() == 1 => match lanes {
            SimdLanes::Int(v) => CtLane::Int(v[0]),
            SimdLanes::Float(v) => CtLane::Float(v[0].to_bits()),
            SimdLanes::Bool(v) => CtLane::Bool(v[0]),
        },
        other => {
            return Err(RuntimeError::TypeError(format!(
                "{}() expects a numeric or Bool value, got {}",
                target.name(),
                type_name(&other)
            )));
        }
    };
    match convert_lane(&lane, target) {
        CtValue::Int(n) => Ok(Value::Int(n)),
        CtValue::UInt(n) => Ok(Value::UInt(n)),
        CtValue::Float(bits) => Ok(Value::Float64(f64::from_bits(bits))),
        CtValue::Bool(b) => Ok(Value::Bool(b)),
        other => Err(RuntimeError::TypeError(format!(
            "{}() produced the non-scalar {other:?}",
            target.name()
        ))),
    }
}

/// `__floor__`/`__ceil__`/`__trunc__` on an integer literal, the one
/// receiver that reaches them by name (a `SIMD`'s are its declared
/// members): the value is already whole.
pub fn builtin_round_dir(method: &str, v: &Value) -> Result<Value, RuntimeError> {
    match v {
        Value::Int(_) | Value::UInt(_) | Value::IntLiteral(_) => Ok(v.clone()),
        other => Err(RuntimeError::TypeError(format!(
            "{}() expects an integer literal, got {}",
            method,
            type_name(other)
        ))),
    }
}

/// The intrinsic behind `math.ceildiv` — the `CeilDivable` dunder
/// (`__ceildiv__(self, denominator) -> Self`, roadmap milestone 7): ceiling
/// division, preserving the operand type.
///
/// Integer ceildiv rounds toward +∞ (`ceildiv(-7, 2) == -3`); float ceildiv is
/// `ceil(a / b)`.
pub fn builtin_ceildiv(a: &Value, b: &Value) -> Result<Value, RuntimeError> {
    match (a, b) {
        // Ceiling = negated floor of the negated numerator (Python flooring).
        (Value::Int(x), Value::Int(y)) => Ok(Value::Int(-floor_div(-*x, nonzero(*y)?))),
        (Value::UInt(x), Value::UInt(y)) => {
            let y = nonzero_u(*y)?;
            Ok(Value::UInt(x / y + u64::from(x % y != 0)))
        }
        (Value::Float64(x), Value::Float64(y)) => Ok(Value::Float64((x / y).ceil())),
        _ => Err(RuntimeError::TypeError(format!(
            "__ceildiv__ expects two numeric values of the same type, got {} and {}",
            type_name(a),
            type_name(b)
        ))),
    }
}

/// Primitive core of `divmod(a, b)` (`DivModable`): compute `(a // b, a % b)`
/// in private pack storage using the operators' flooring rules.
///
/// The VM wraps this transient value in the checker-selected nominal public
/// Tuple before it crosses the instruction-result boundary.
pub fn builtin_divmod(a: Value, b: Value) -> Result<Value, RuntimeError> {
    let q = apply_infix(InfixOp::FloorDiv, a.clone(), b.clone())?;
    let r = apply_infix(InfixOp::Mod, a, b)?;
    Ok(Value::Tuple(vec![q, r]))
}

/// `Error(msg)`: wrap a `String` into an `Error` value.
pub fn builtin_error(v: Value) -> Result<Value, RuntimeError> {
    match v {
        Value::Str(s) => Ok(Value::Error(s)),
        other => Err(RuntimeError::TypeError(format!(
            "Error() expects a String, got {}",
            type_name(&other)
        ))),
    }
}

/// Read lane `i` of a SIMD as a width-1 SIMD (scalar) of the same dtype — the
/// shared core of `v[i]` reads and SIMD-lane read-modify-write.
///
/// Build a SIMD value of `dtype`/`width` from element `values`: exactly
/// `width` elements (one per lane) or a single element splatted to every lane.
pub fn simd_from_values(
    dtype: Dtype,
    width: usize,
    values: &[Value],
) -> Result<Value, RuntimeError> {
    // A single argument splats; otherwise one argument per lane.
    let pick = |i: usize| &values[if values.len() == 1 { 0 } else { i }];
    let lanes = if dtype == Dtype::Bool {
        let mut v = Vec::with_capacity(width);
        for i in 0..width {
            v.push(value_to_bool_lane(pick(i))?);
        }
        SimdLanes::Bool(v)
    } else if dtype.is_float() {
        let mut v = Vec::with_capacity(width);
        for i in 0..width {
            v.push(value_to_float_lane(pick(i), dtype)?);
        }
        SimdLanes::Float(v)
    } else {
        let mut v = Vec::with_capacity(width);
        for i in 0..width {
            v.push(value_to_int_lane(pick(i), dtype)?);
        }
        SimdLanes::Int(v)
    };
    Ok(simd_value(dtype, lanes))
}

pub fn read_simd_lane(dtype: Dtype, lanes: &SimdLanes, i: i64) -> Result<Value, RuntimeError> {
    let idx = bounds_check(i, lanes.width(), "SIMD lane")?;
    let lane = match lanes {
        SimdLanes::Int(v) => SimdLanes::Int(vec![v[idx]]),
        SimdLanes::Float(v) => SimdLanes::Float(vec![v[idx]]),
        SimdLanes::Bool(v) => SimdLanes::Bool(vec![v[idx]]),
    };
    Ok(simd_value(dtype, lane))
}

/// Write scalar `value` (or a splatting literal) into lane `i`, wrapping to the
/// element width exactly as construction does (`wrap`/`Dtype::round_lane`).
pub fn set_simd_lane(
    dtype: Dtype,
    lanes: &mut SimdLanes,
    i: i64,
    value: &Value,
) -> Result<(), RuntimeError> {
    let idx = bounds_check(i, lanes.width(), "SIMD lane")?;
    match lanes {
        SimdLanes::Int(v) => v[idx] = value_to_int_lane(value, dtype)?,
        SimdLanes::Float(v) => v[idx] = value_to_float_lane(value, dtype)?,
        SimdLanes::Bool(v) => v[idx] = value_to_bool_lane(value)?,
    }
    Ok(())
}

/// A SIMD value's dtype and lanes, whether it is held as lanes or as the host
/// scalar of a width-one `Int`, `UInt`, or `Float64`.
pub fn simd_parts(value: &Value) -> Option<(Dtype, SimdLanes)> {
    match value {
        Value::Simd { dtype, lanes } => Some((*dtype, lanes.clone())),
        other => host_scalar_parts(other),
    }
}

/// Read lane `i` of a SIMD value, or `None` when `value` is no SIMD value.
pub fn read_value_lane(value: &Value, i: i64) -> Option<Result<Value, RuntimeError>> {
    match value {
        Value::Simd { dtype, lanes } => Some(read_simd_lane(*dtype, lanes, i)),
        other => host_scalar_parts(other).map(|(dtype, lanes)| read_simd_lane(dtype, &lanes, i)),
    }
}

/// Write lane `i` of the SIMD value `target` in place, or `None` when
/// `target` is no SIMD value. A width-one host scalar is replaced whole.
pub fn set_value_lane(
    target: &mut Value,
    i: i64,
    value: &Value,
) -> Option<Result<(), RuntimeError>> {
    if let Value::Simd { dtype, lanes } = target {
        return Some(set_simd_lane(*dtype, lanes, i, value));
    }
    let (dtype, mut lanes) = host_scalar_parts(target)?;
    Some(set_simd_lane(dtype, &mut lanes, i, value).map(|()| *target = simd_value(dtype, lanes)))
}

/// Apply an elementwise SIMD operator.
///
/// One operand may be a scalar that splats to the other's dtype and width.
/// Integer arithmetic is bit-accurate; `float32` rounds each result to single
/// precision. Comparisons yield a `bool` mask. (The checker guarantees
/// dtype/width agreement and operator validity.)
pub fn simd_binop(op: InfixOp, l: &Value, r: &Value) -> Result<Value, RuntimeError> {
    use InfixOp::{Eq, Ge, Gt, Le, Lt, Ne};
    // The result takes the wider operand's width: a width-1 SIMD operand (a
    // `Int32`-typed scalar) splats exactly like a bare scalar.
    let (dtype, width) = match (simd_shape(l), simd_shape(r)) {
        (Some((dtype, left)), Some((_, right))) => (dtype, left.max(right)),
        (Some(shape), None) | (None, Some(shape)) => shape,
        (None, None) => {
            return Err(RuntimeError::TypeError(
                "SIMD dispatch requires at least one SIMD operand".to_string(),
            ));
        }
    };
    let is_cmp = matches!(op, Eq | Ne | Lt | Gt | Le | Ge);
    match dtype {
        Dtype::Bool => {
            let xs = to_bool_lanes(l, width)?;
            let ys = to_bool_lanes(r, width)?;
            let out: Vec<bool> = xs
                .iter()
                .zip(&ys)
                // Only the `SIMD.lt`/`le`/`gt`/`ge` methods reach the ordered
                // comparisons: the checker rejects the infix operators, and
                // every other operator on bool lanes.
                .map(|(a, b)| bool_lane_binop(op, *a, *b).unwrap_or(false))
                .collect();
            Ok(Value::Simd {
                dtype: Dtype::Bool,
                lanes: SimdLanes::Bool(out),
            })
        }
        d if d.is_float() => {
            let xs = to_float_lanes(l, dtype, width)?;
            let ys = to_float_lanes(r, dtype, width)?;
            if is_cmp {
                let out = xs
                    .iter()
                    .zip(&ys)
                    .map(|(a, b)| lane_compare(op, *a, *b).unwrap_or(false))
                    .collect();
                Ok(Value::Simd {
                    dtype: Dtype::Bool,
                    lanes: SimdLanes::Bool(out),
                })
            } else {
                let out: Vec<f64> = xs
                    .iter()
                    .zip(&ys)
                    .map(|(a, b)| float_lane_binop(op, dtype, *a, *b).unwrap_or(0.0))
                    .collect();
                Ok(Value::Simd {
                    dtype,
                    lanes: SimdLanes::Float(out),
                })
            }
        }
        d => {
            let xs = to_int_lanes(l, d, width)?;
            let ys = to_int_lanes(r, d, width)?;
            if is_cmp {
                let out = xs
                    .iter()
                    .zip(&ys)
                    .map(|(a, b)| lane_compare(op, *a, *b).unwrap_or(false))
                    .collect();
                Ok(Value::Simd {
                    dtype: Dtype::Bool,
                    lanes: SimdLanes::Bool(out),
                })
            } else {
                if op == InfixOp::Div && ys.contains(&0) {
                    return Err(RuntimeError::TypeError(
                        "integer division or modulo by zero".to_string(),
                    ));
                }
                let out: Vec<i128> = xs
                    .iter()
                    .zip(&ys)
                    .map(|(a, b)| int_lane_binop(op, d, *a, *b).unwrap_or(0))
                    .collect();
                Ok(Value::Simd {
                    dtype: d,
                    lanes: SimdLanes::Int(out),
                })
            }
        }
    }
}

/// Compute upstream's `pop` lane op `op` over `operands`, SIMD values of one
/// type.
///
/// A width-one `Int`, `UInt`, or `Float64` held as its host scalar stays
/// one. Each lane is `mlir::pop_int_lane`, `mlir::pop_float_lane`, or
/// `mlir::pop_bool_lane`, and an integer division by zero is an error, as
/// the scalar operator's is. An unsigned compare of two `index` values,
/// held as `Int`s, gives a `Bool`.
pub fn pop_op(op: mojito_types::mlir::PopOp, operands: &[Value]) -> Result<Value, RuntimeError> {
    let mismatch = || {
        RuntimeError::TypeError(format!(
            "'pop.{}' takes operands of one SIMD type",
            op.mnemonic()
        ))
    };
    let parts = operands
        .iter()
        .map(|operand| simd_parts(operand).ok_or_else(mismatch))
        .collect::<Result<Vec<_>, _>>()?;
    let (dtype, width) = match parts.first() {
        Some((dtype, lanes)) => (*dtype, lanes.width()),
        None => return Err(mismatch()),
    };
    let lanes = if dtype.is_float() {
        SimdLanes::Float(zip_lanes(&parts, width, float_lanes, mismatch, |lane| {
            mojito_types::mlir::pop_float_lane(op, dtype, lane).ok_or_else(mismatch)
        })?)
    } else if dtype == Dtype::Bool {
        SimdLanes::Bool(zip_lanes(
            &parts,
            width,
            bool_lanes,
            mismatch,
            |lane| match lane {
                [a, b] => mojito_types::mlir::pop_bool_lane(op, *a, *b).ok_or_else(mismatch),
                _ => Err(mismatch()),
            },
        )?)
    } else if op.is_compare() {
        let lanes = zip_lanes(&parts, width, int_lanes, mismatch, |lane| {
            mojito_types::mlir::pop_int_lane(op, dtype, lane).ok_or_else(mismatch)
        })?;
        return match lanes.as_slice() {
            [lane] => Ok(Value::Bool(*lane != 0)),
            _ => Err(mismatch()),
        };
    } else {
        SimdLanes::Int(zip_lanes(&parts, width, int_lanes, mismatch, |lane| {
            mojito_types::mlir::pop_int_lane(op, dtype, lane).ok_or_else(|| {
                RuntimeError::TypeError("integer division or modulo by zero".to_string())
            })
        })?)
    };
    Ok(simd_value(dtype, lanes))
}

/// Call the LLVM intrinsic `intrinsic` over `operands`, giving a `result`.
///
/// Lanes are computed as `mojito_types::llvm_intrinsic` computes them: an
/// elementwise intrinsic gives its first operand's type, `is.fpclass` a
/// `bool` lane per operand lane, a reduction one
/// lane of it, a shuffle the lanes its sources name, `llvm.stepvector` the
/// result's lane positions, and `llvm.donothing` `None`. `result` is the
/// type LLVM translation sees (`MirDeclarations::translated_type`): a
/// literal struct result (`vector.deinterleave2`'s) is the tuple of its
/// elements' lanes, each a slice of the sources.
pub fn llvm_intrinsic(
    intrinsic: mojito_types::llvm_intrinsic::LlvmIntrinsic,
    result: Option<&Ty>,
    operands: &[Value],
) -> Result<Value, RuntimeError> {
    use mojito_types::llvm_intrinsic::{self as llvm, LlvmIntrinsic};
    let mismatch = || {
        RuntimeError::TypeError(format!(
            "'{}' does not take these operands",
            intrinsic.name()
        ))
    };
    let elements = match result.map(Ty::kind) {
        Some(mojito_types::types::TyKind::Tuple(elements)) => Some(elements.as_slice()),
        _ => None,
    };
    let result_lanes = || match elements
        .and_then(<[Ty]>::first)
        .or(result)
        .map(Ty::kind)
        .and_then(mojito_types::types::simd_slots)
    {
        Some((
            mojito_types::types::SimdDtype::Known(dtype),
            mojito_types::types::SimdWidth::Known(width),
        )) => usize::try_from(width).ok().map(|width| (dtype, width)),
        _ => None,
    };
    match intrinsic {
        LlvmIntrinsic::Donothing => return Ok(Value::None),
        LlvmIntrinsic::Stepvector => {
            let (dtype, width) = result_lanes().ok_or_else(mismatch)?;
            return Ok(simd_value(
                dtype,
                SimdLanes::Int(llvm::step_lanes(dtype, width)),
            ));
        }
        _ => {}
    }
    let index = intrinsic.index_operand().map(|(position, _)| position);
    let parts = operands
        .iter()
        .enumerate()
        .filter(|(position, _)| !intrinsic.is_flag(*position) && index != Some(*position))
        .map(|(_, operand)| simd_parts(operand).ok_or_else(mismatch))
        .collect::<Result<Vec<_>, _>>()?;
    let (dtype, width) = match parts.first() {
        Some((dtype, lanes)) => (*dtype, lanes.width()),
        None => return Err(mismatch()),
    };
    let index = match index {
        Some(position) => match operands.get(position).and_then(simd_parts) {
            Some((_, SimdLanes::Int(lanes))) if lanes.len() == 1 => lanes[0],
            _ => return Err(mismatch()),
        },
        None => 0,
    };
    let result_width = result_lanes().map_or(width, |(_, width)| width);
    let widths: Vec<usize> = parts.iter().map(|(_, lanes)| lanes.width()).collect();
    if let Some(reason) = intrinsic.index_error(result_width, &widths, index) {
        return Err(RuntimeError::Unsupported(reason));
    }
    if let Some(sources) = intrinsic.shuffle_sources(result_width, &widths, index) {
        if elements.is_some() {
            return sources
                .chunks(result_width.max(1))
                .map(|element| {
                    shuffle_lanes(&parts, element)
                        .map(|lanes| simd_value(dtype, lanes))
                        .ok_or_else(mismatch)
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Tuple);
        }
        return shuffle_lanes(&parts, &sources)
            .map(|lanes| simd_value(dtype, lanes))
            .ok_or_else(mismatch);
    }
    if intrinsic == LlvmIntrinsic::IsFpclass {
        let test = u32::try_from(index).map_err(|_| mismatch())?;
        let lanes = float_lanes(&parts[0].1).ok_or_else(mismatch)?;
        return Ok(simd_value(
            Dtype::Bool,
            SimdLanes::Bool(
                lanes
                    .iter()
                    .map(|lane| llvm::fpclass_lane(dtype, *lane, test))
                    .collect(),
            ),
        ));
    }
    let lanes = match (intrinsic, &parts[0].1) {
        (LlvmIntrinsic::VectorReduce(reduction), SimdLanes::Int(lanes)) => SimdLanes::Int(vec![
            llvm::reduce_ints(reduction, dtype, lanes).ok_or_else(mismatch)?,
        ]),
        (LlvmIntrinsic::VectorReduce(reduction), SimdLanes::Float(lanes)) => {
            SimdLanes::Float(vec![
                llvm::reduce_floats(reduction, lanes).ok_or_else(mismatch)?,
            ])
        }
        (_, SimdLanes::Int(_)) => {
            SimdLanes::Int(zip_lanes(&parts, width, int_lanes, mismatch, |lane| {
                llvm::int_lane(intrinsic, dtype, lane).ok_or_else(mismatch)
            })?)
        }
        (_, SimdLanes::Float(_)) => {
            SimdLanes::Float(zip_lanes(&parts, width, float_lanes, mismatch, |lane| {
                llvm::float_lane(intrinsic, dtype, lane).ok_or_else(mismatch)
            })?)
        }
        (_, SimdLanes::Bool(_)) => return Err(mismatch()),
    };
    Ok(simd_value(dtype, lanes))
}

/// Coerce a value to a declared type at a binding site. Only materializes a
/// numeric literal's default (`Int`/`Float64`) into another numeric type; all
/// other values pass through unchanged.
///
/// # Panics
///
/// Panics if a literal that reached the VM does not round to `binary64`, which
/// the lexer's literal grammar rules out.
pub fn coerce(value: Value, ty: &Type) -> Value {
    match ty {
        Type::UInt => match value {
            Value::Int(n) => Value::UInt(n as u64),
            Value::IntLiteral(value) => value
                .to_u64()
                .map_or_else(|| Value::IntLiteral(value), Value::UInt),
            v => v,
        },
        Type::Float64 => match value {
            Value::Int(n) => Value::Float64(n as f64),
            Value::UInt(n) => Value::Float64(n as f64),
            Value::IntLiteral(value) => Value::Float64(
                value
                    .to_f64()
                    .expect("an integer literal always rounds to binary64"),
            ),
            Value::FloatLiteral(value) => Value::Float64(
                value
                    .to_f64()
                    .expect("a finite float literal always rounds to binary64"),
            ),
            v => v,
        },
        _ => value,
    }
}

/// Apply the exact literal-to-runtime conversion selected by checked HIR and
/// represented explicitly in MIR.
pub fn materialize_literal(
    value: Value,
    target: &mojito_types::types::Ty,
) -> Result<Value, RuntimeError> {
    use mojito_types::types::TyKind;
    match (value, target.kind()) {
        (Value::IntLiteral(value), TyKind::INT) => value
            .wrapping_signed(64)
            .map(Value::Int)
            .ok_or_else(|| literal_materialization_error(&value, "Int")),
        (Value::IntLiteral(value), TyKind::UINT) => value
            .wrapping_unsigned(64)
            .map(Value::UInt)
            .ok_or_else(|| literal_materialization_error(&value, "UInt")),
        (Value::IntLiteral(value), TyKind::FLOAT64) => value
            .to_f64()
            .map(Value::Float64)
            .ok_or_else(|| literal_materialization_error(&value, "Float64")),
        (Value::FloatLiteral(value), TyKind::FLOAT64) => value
            .to_f64()
            .map(Value::Float64)
            .ok_or_else(|| literal_materialization_error(&value, "Float64")),
        (Value::IntLiteral(value), target)
            if let Some(dtype) = mojito_types::types::scalar_simd_dtype(target) =>
        {
            CtLane::of_int_literal(&value, dtype)
                .map(|lane| scalar_lane_value(dtype, &lane))
                .ok_or_else(|| literal_materialization_error(&value, &format!("{dtype:?}")))
        }
        (Value::FloatLiteral(value), target)
            if let Some(dtype) = mojito_types::types::scalar_simd_dtype(target) =>
        {
            CtLane::of_float_literal(&value, dtype)
                .map(|lane| scalar_lane_value(dtype, &lane))
                .ok_or_else(|| literal_materialization_error(&value, &format!("{dtype:?}")))
        }
        // Upstream's implicit `SIMD(IntLiteral)`/`(FloatLiteral)` splat: the
        // literal materializes as one lane, then fills the vector. It is the
        // run-time form of the splat `simd.make` that `sccp` folds.
        (value @ (Value::IntLiteral(_) | Value::FloatLiteral(_)), target)
            if let Some((dtype, width)) = mojito_types::types::simd_shape(target)
                && let Ok(width) = usize::try_from(width)
                && width > 1 =>
        {
            let lane =
                materialize_literal(value, &mojito_types::types::canonical_simd_ty(dtype, 1))?;
            simd_from_values(dtype, width, &[lane])
        }
        (value, target) => Err(RuntimeError::TypeError(format!(
            "cannot materialize {} as {target}",
            type_name(&value)
        ))),
    }
}

pub fn coerce_checked(value: Value, ty: &mojito_types::types::Ty) -> Value {
    use mojito_types::types::TyKind;
    if matches!(value, Value::IntLiteral(_) | Value::FloatLiteral(_))
        && let Ok(materialized) = materialize_literal(value.clone(), ty)
    {
        return materialized;
    }
    match ty.kind() {
        TyKind::UINT => match value {
            Value::Int(n) => Value::UInt(n as u64),
            value => value,
        },
        TyKind::FLOAT64 => match value {
            Value::Int(n) => Value::Float64(n as f64),
            Value::UInt(n) => Value::Float64(n as f64),
            value => value,
        },
        ty if let Some(dtype) = mojito_types::types::scalar_simd_dtype(ty) => {
            match simd_from_values(dtype, 1, std::slice::from_ref(&value)) {
                Ok(materialized) => materialized,
                Err(_) => value,
            }
        }
        TyKind::Tuple(types) => match value {
            Value::Tuple(items) => Value::Tuple(
                items
                    .into_iter()
                    .zip(types)
                    .map(|(value, ty)| coerce_checked(value, ty))
                    .collect(),
            ),
            value => value,
        },
        TyKind::ComptimeList(element) => match value {
            Value::ComptimeList(items) => Value::ComptimeList(
                items
                    .into_iter()
                    .map(|value| coerce_checked(value, element))
                    .collect(),
            ),
            value => value,
        },
        TyKind::Variant(types) => match value {
            Value::Variant {
                alternatives,
                index,
                value,
            } if alternatives == *types && index < types.len() => Value::Variant {
                alternatives,
                index,
                value: Box::new(coerce_checked(*value, &types[index])),
            },
            value => value,
        },
        _ => value,
    }
}

/// Coerce a value to match an existing binding's numeric type (for assignment,
/// where the declared type isn't carried but the current value's type is it).
pub fn coerce_like(new: Value, existing: &Value) -> Value {
    if matches!(new, Value::IntLiteral(_) | Value::FloatLiteral(_))
        && let Ok(materialized) = materialize_like_numeric(new.clone(), existing)
    {
        return materialized;
    }
    match existing {
        Value::UInt(_) => coerce(new, &Type::UInt),
        Value::Float64(_) => coerce(new, &Type::Float64),
        _ => new,
    }
}

/// The one-lane SIMD value of a materialized compile-time lane.
fn scalar_lane_value(dtype: Dtype, lane: &CtLane) -> Value {
    simd_value(
        dtype,
        match lane {
            CtLane::Int(value) => SimdLanes::Int(vec![*value]),
            CtLane::Float(bits) => SimdLanes::Float(vec![f64::from_bits(*bits)]),
            CtLane::Bool(value) => SimdLanes::Bool(vec![*value]),
        },
    )
}

fn literal_materialization_error(value: &impl fmt::Display, destination: &str) -> RuntimeError {
    RuntimeError::TypeError(format!(
        "literal value {value} cannot be represented as {destination}"
    ))
}

/// A numeric value lifted out of `Value` for arithmetic. Operands are promoted to
/// a common kind before an operator is applied (`Int < UInt < Float64`).
#[derive(Clone, Copy)]
enum Num {
    I(i64),
    U(u64),
    F(f64),
}

impl Num {
    const fn rank(self) -> u8 {
        match self {
            Self::I(_) => 0,
            Self::U(_) => 1,
            Self::F(_) => 2,
        }
    }
    const fn as_i64(self) -> i64 {
        match self {
            Self::I(n) => n,
            Self::U(n) => n as i64,
            Self::F(x) => x as i64,
        }
    }
    const fn as_u64(self) -> u64 {
        match self {
            Self::I(n) => n as u64,
            Self::U(n) => n,
            Self::F(x) => x as u64,
        }
    }
    const fn as_f64(self) -> f64 {
        match self {
            Self::I(n) => n as f64,
            Self::U(n) => n as f64,
            Self::F(x) => x,
        }
    }
}

/// View a value as a number, if it is one.
const fn as_num(v: &Value) -> Option<Num> {
    match &v {
        Value::Int(n) => Some(Num::I(*n)),
        Value::UInt(n) => Some(Num::U(*n)),
        Value::Float64(x) => Some(Num::F(*x)),
        _ => None,
    }
}

#[derive(Clone)]
enum ExactNum {
    Int(mojito_common::literal::IntLiteral),
    Float(mojito_common::literal::FloatLiteral),
}

impl ExactNum {
    fn into_float(self) -> mojito_common::literal::FloatLiteral {
        match self {
            Self::Int(value) => mojito_common::literal::FloatLiteral::from_int(&value),
            Self::Float(value) => value,
        }
    }

    fn compare(&self, rhs: &Self) -> Ordering {
        match (self, rhs) {
            (Self::Int(left), Self::Int(right)) => left.cmp(right),
            (left, right) => left
                .clone()
                .into_float()
                .numeric_cmp(&right.clone().into_float()),
        }
    }
}

fn as_exact_num(value: &Value) -> Option<ExactNum> {
    match value {
        Value::IntLiteral(value) => Some(ExactNum::Int(value.clone())),
        Value::FloatLiteral(value) => Some(ExactNum::Float(value.clone())),
        _ => None,
    }
}

fn materialize_like_numeric(literal: Value, concrete: &Value) -> Result<Value, RuntimeError> {
    match (literal, concrete) {
        (Value::IntLiteral(value), Value::Int(_)) => value
            .wrapping_signed(64)
            .map(Value::Int)
            .ok_or_else(|| literal_materialization_error(&value, "Int")),
        (Value::IntLiteral(value), Value::UInt(_)) => value
            .wrapping_unsigned(64)
            .map(Value::UInt)
            .ok_or_else(|| literal_materialization_error(&value, "UInt")),
        (Value::IntLiteral(value), Value::Float64(_)) => value
            .to_f64()
            .map(Value::Float64)
            .ok_or_else(|| literal_materialization_error(&value, "Float64")),
        (Value::FloatLiteral(value), Value::Float64(_)) => value
            .to_f64()
            .map(Value::Float64)
            .ok_or_else(|| literal_materialization_error(&value, "Float64")),
        (value, _) => Err(RuntimeError::TypeError(format!(
            "cannot materialize {} for {}",
            type_name(&value),
            type_name(concrete)
        ))),
    }
}

/// The one lane of a width-one `Int`, `UInt`, or `Float64` host scalar,
/// and of a host `Bool`, which is a one-lane mask: `n.eq(0)` over an `Int`
/// compares host scalars into a `Bool` that `select` reads as its mask, the
/// representation `builtin.cast` converts to and from `Scalar[DType.bool]`.
fn host_scalar_parts(value: &Value) -> Option<(Dtype, SimdLanes)> {
    match value {
        Value::Int(n) => Some((Dtype::Int, SimdLanes::Int(vec![i128::from(*n)]))),
        Value::UInt(n) => Some((Dtype::UInt, SimdLanes::Int(vec![i128::from(*n)]))),
        Value::Float64(x) => Some((Dtype::Float64, SimdLanes::Float(vec![*x]))),
        Value::Bool(b) => Some((Dtype::Bool, SimdLanes::Bool(vec![*b]))),
        _ => None,
    }
}

/// Lane `index` of each operand's lanes, which `pick` reads, computed by
/// `lane` over them in operand order, for each of `width` lanes; an operand
/// `pick` refuses or a short one is `mismatch`.
/// The lanes `sources` name among `parts`' lanes laid end to end, a `None`
/// source a zero lane; `None` when the parts' lanes differ in kind or a
/// source is past them.
fn shuffle_lanes(parts: &[(Dtype, SimdLanes)], sources: &[Option<usize>]) -> Option<SimdLanes> {
    fn pick<T: Copy>(
        parts: &[(Dtype, SimdLanes)],
        sources: &[Option<usize>],
        lanes: fn(&SimdLanes) -> Option<&[T]>,
        zero: T,
    ) -> Option<Vec<T>> {
        let concatenated = parts
            .iter()
            .map(|(_, part)| lanes(part))
            .collect::<Option<Vec<_>>>()?
            .concat();
        sources
            .iter()
            .map(|source| source.map_or(Some(zero), |source| concatenated.get(source).copied()))
            .collect()
    }
    Some(match parts.first().map(|(_, lanes)| lanes)? {
        SimdLanes::Int(_) => SimdLanes::Int(pick(parts, sources, int_lanes, 0)?),
        SimdLanes::Float(_) => SimdLanes::Float(pick(parts, sources, float_lanes, 0.0)?),
        SimdLanes::Bool(_) => SimdLanes::Bool(pick(parts, sources, bool_lanes, false)?),
    })
}

fn zip_lanes<T: Copy>(
    parts: &[(Dtype, SimdLanes)],
    width: usize,
    pick: fn(&SimdLanes) -> Option<&[T]>,
    mismatch: impl Fn() -> RuntimeError,
    lane: impl Fn(&[T]) -> Result<T, RuntimeError>,
) -> Result<Vec<T>, RuntimeError> {
    (0..width)
        .map(|index| {
            let operands = parts
                .iter()
                .map(|(_, lanes)| pick(lanes).and_then(|values| values.get(index).copied()))
                .collect::<Option<Vec<T>>>()
                .ok_or_else(&mismatch)?;
            lane(&operands)
        })
        .collect()
}

fn int_lanes(lanes: &SimdLanes) -> Option<&[i128]> {
    match lanes {
        SimdLanes::Int(values) => Some(values),
        _ => None,
    }
}

fn float_lanes(lanes: &SimdLanes) -> Option<&[f64]> {
    match lanes {
        SimdLanes::Float(values) => Some(values),
        _ => None,
    }
}

fn bool_lanes(lanes: &SimdLanes) -> Option<&[bool]> {
    match lanes {
        SimdLanes::Bool(values) => Some(values),
        _ => None,
    }
}

fn simd_value(dtype: Dtype, lanes: SimdLanes) -> Value {
    if dtype == Dtype::Float64
        && let SimdLanes::Float(v) = &lanes
        && v.len() == 1
    {
        return Value::Float64(v[0]);
    }
    if dtype == Dtype::Int
        && let SimdLanes::Int(v) = &lanes
        && v.len() == 1
    {
        return Value::Int(v[0] as i64);
    }
    if dtype == Dtype::UInt
        && let SimdLanes::Int(v) = &lanes
        && v.len() == 1
    {
        return Value::UInt(v[0] as u64);
    }
    Value::Simd { dtype, lanes }
}

/// Lift any finite runtime numeric value into the same exact comparison domain
/// as a literal. This keeps generic equality and hashing coherent even
/// if an exact literal survives past a boundary that normally materializes it.
fn as_finite_numeric(value: &Value) -> Option<ExactNum> {
    match value {
        Value::Int(value) => Some(ExactNum::Int((*value).into())),
        Value::UInt(value) => Some(ExactNum::Int((*value).into())),
        Value::Float64(value) => {
            mojito_common::literal::FloatLiteral::from_f64(*value).map(ExactNum::Float)
        }
        value => as_exact_num(value),
    }
}

fn nonzero(y: i64) -> Result<i64, RuntimeError> {
    (y != 0)
        .then_some(y)
        .ok_or_else(|| RuntimeError::TypeError("integer division or modulo by zero".to_string()))
}

/// Python/Mojo floor division: round toward negative infinity. The one
/// overflowing case, `i64::MIN // -1`, is defined to wrap to `i64::MIN`
/// (its remainder is 0, so no floor adjustment applies).
const fn floor_div(x: i64, y: i64) -> i64 {
    let q = x.wrapping_div(y);
    let r = x.wrapping_rem(y);
    if r != 0 && ((r < 0) != (y < 0)) {
        q - 1
    } else {
        q
    }
}

fn nonzero_u(y: u64) -> Result<u64, RuntimeError> {
    (y != 0)
        .then_some(y)
        .ok_or_else(|| RuntimeError::TypeError("integer division or modulo by zero".to_string()))
}

/// Python/Mojo modulo: the result takes the sign of the divisor.
/// `i64::MIN % -1` is 0 (see `floor_div` on the wrapped quotient).
const fn floor_mod(x: i64, y: i64) -> i64 {
    let r = x.wrapping_rem(y);
    if r != 0 && ((r < 0) != (y < 0)) {
        r + y
    } else {
        r
    }
}

fn value_to_int_lane(v: &Value, dtype: Dtype) -> Result<i128, RuntimeError> {
    if let Value::IntLiteral(value) = v {
        if integer_dtype_bits(dtype).is_none() {
            return Err(RuntimeError::TypeError(format!(
                "{dtype:?} is not an integer SIMD dtype"
            )));
        }
        return match CtLane::of_int_literal(value, dtype) {
            Some(CtLane::Int(lane)) => Ok(lane),
            _ => Err(literal_materialization_error(value, &format!("{dtype:?}"))),
        };
    }
    Ok(wrap_lane(dtype, value_to_int(v)?))
}

/// A scalar value's floating content, for building a float SIMD lane.
fn value_to_float(v: &Value) -> Result<f64, RuntimeError> {
    match v {
        Value::Int(n) => Ok(*n as f64),
        Value::UInt(n) => Ok(*n as f64),
        Value::Float64(x) => Ok(*x),
        Value::IntLiteral(value) => value
            .to_f64()
            .ok_or_else(|| literal_materialization_error(value, "floating SIMD lane")),
        Value::FloatLiteral(value) => value
            .to_f64()
            .ok_or_else(|| literal_materialization_error(value, "floating SIMD lane")),
        Value::Simd {
            lanes: SimdLanes::Float(l),
            ..
        } if l.len() == 1 => Ok(l[0]),
        // An integer scalar converts to a float lane at explicit construction.
        Value::Simd {
            lanes: SimdLanes::Int(l),
            ..
        } if l.len() == 1 => Ok(l[0] as f64),
        other => Err(RuntimeError::TypeError(format!(
            "cannot use {} as a float SIMD element",
            type_name(other)
        ))),
    }
}

fn value_to_float_lane(v: &Value, dtype: Dtype) -> Result<f64, RuntimeError> {
    let alias = dtype.scalar_alias().unwrap_or("floating SIMD lane");
    let lane = |lane: Option<CtLane>| match lane {
        Some(CtLane::Float(bits)) => Some(f64::from_bits(bits)),
        _ => None,
    };
    match v {
        Value::IntLiteral(value) => lane(CtLane::of_int_literal(value, dtype))
            .ok_or_else(|| literal_materialization_error(value, alias)),
        Value::FloatLiteral(value) => lane(CtLane::of_float_literal(value, dtype))
            .ok_or_else(|| literal_materialization_error(value, alias)),
        other => Ok(dtype.round_lane(value_to_float(other)?)),
    }
}

/// A scalar value's boolean content, for building a `bool` SIMD lane.
fn value_to_bool_lane(v: &Value) -> Result<bool, RuntimeError> {
    match v {
        Value::Bool(b) => Ok(*b),
        // Upstream's `IntLiteral` initializer: a nonzero literal is `True`.
        Value::IntLiteral(value) => Ok(!value.is_zero()),
        Value::Simd {
            lanes: SimdLanes::Bool(l),
            ..
        } if l.len() == 1 => Ok(l[0]),
        other => Err(RuntimeError::TypeError(format!(
            "cannot use {} as a bool SIMD element",
            type_name(other)
        ))),
    }
}

/// Validate an integer exponent for `**` (non-negative and `u32`-sized).
///
/// `0 ..= u32::MAX` is the range the native backend traps outside of (trap
/// category 2) before calling the same `_pow_int` body.
pub fn pow_exponent(y: i64) -> Result<u32, RuntimeError> {
    u32::try_from(y).map_err(|_| {
        RuntimeError::TypeError(
            "'**' exponent must be a non-negative Int that fits in 32 bits".to_string(),
        )
    })
}

/// The `(unsigned, base, exponent)` bits of an integer `**`.
///
/// Both operands must be runtime integers — a literal operand takes the
/// other's signedness, as [`apply_infix`]'s materialization would. `None` for
/// every other pair, which leaves float `**`, literal-only `**`, and
/// non-numeric operands on their own paths; the caller runs the bundled
/// `_pow_int` body on what this accepts.
pub fn integer_pow_operands(left: &Value, right: &Value) -> Option<(bool, i64, i64)> {
    enum Operand {
        Runtime(Num),
        Literal(i64),
    }
    let operand = |value: &Value| match value {
        Value::Int(n) => Some(Operand::Runtime(Num::I(*n))),
        Value::UInt(n) => Some(Operand::Runtime(Num::U(*n))),
        Value::IntLiteral(literal) => literal.wrapping_signed(64).map(Operand::Literal),
        _ => None,
    };
    let (left, right) = (operand(left)?, operand(right)?);
    let unsigned = match (&left, &right) {
        (Operand::Literal(_), Operand::Literal(_)) => return None,
        (Operand::Runtime(num), _) | (_, Operand::Runtime(num)) => num.rank() == 1,
    };
    let bits = |operand: &Operand| match operand {
        Operand::Runtime(num) => num.as_i64(),
        Operand::Literal(value) => *value,
    };
    Some((unsigned, bits(&left), bits(&right)))
}

/// The `(bits, signed)` lane shape of an integer SIMD dtype, or `None` for
/// the float and bool dtypes. Shared with the native backends so their width
/// tables cannot drift from the VM's.
pub const fn integer_dtype_bits(dtype: Dtype) -> Option<(u32, bool)> {
    Some(match dtype {
        Dtype::Int => (64, true),
        Dtype::Int8 => (8, true),
        Dtype::Int16 => (16, true),
        Dtype::Int32 => (32, true),
        Dtype::Int64 => (64, true),
        Dtype::UInt8 => (8, false),
        Dtype::UInt16 => (16, false),
        Dtype::UInt32 => (32, false),
        Dtype::UInt | Dtype::UInt64 => (64, false),
        Dtype::Float16 | Dtype::Float32 | Dtype::Float64 | Dtype::Bool => return None,
    })
}

/// The `(dtype, width)` of a value if it is a SIMD.
const fn simd_shape(v: &Value) -> Option<(Dtype, usize)> {
    match v {
        Value::Simd { dtype, lanes } => Some((*dtype, lanes.width())),
        _ => None,
    }
}

/// Refuse the compile-time cast of `value` to the integer `target` when a
/// float lane of it is NaN or outside the target's range.
///
/// The pin's comptime interpreter refuses the poison LLVM's
/// `fptosi`/`fptoui` yields there; a run-time cast saturates.
pub fn ctfe_float_to_int(target: Dtype, value: &Value) -> Result<(), RuntimeError> {
    if !target.is_integral() {
        return Ok(());
    }
    let Some((_, SimdLanes::Float(lanes))) = simd_parts(value) else {
        return Ok(());
    };
    let bits = i32::try_from(target.bit_width()).unwrap_or(i32::MAX);
    let (low, high) = if target.is_signed() {
        (-(2f64.powi(bits - 1)), 2f64.powi(bits - 1))
    } else {
        (0.0, 2f64.powi(bits))
    };
    match lanes
        .iter()
        .find(|lane| !(lane.is_finite() && lane.trunc() >= low && lane.trunc() < high))
    {
        Some(lane) => Err(RuntimeError::Unsupported(format!(
            "the conversion of {lane} to {} is not a compile-time value",
            target.name()
        ))),
        None => Ok(()),
    }
}

/// Elementwise dtype conversion, upstream's `pop.cast`.
///
/// Integer targets re-wrap at the new element width, float targets convert
/// numerically (`float32` rounds), and a float source converts to an integer
/// lane by truncating toward zero (saturating at the 128-bit intermediate) and
/// then wrapping — Mojito's pinned bit-accurate policy. A `bool` lane widens
/// to 0 or 1, and a `bool` target takes the low bit (`trunc` to `i1`) of the
/// integer lane or of the float lane truncated.
pub fn simd_cast(target: Dtype, value: &Value) -> Result<Value, RuntimeError> {
    let (_, lanes) = simd_parts(value).ok_or_else(|| {
        RuntimeError::TypeError(format!("cannot cast {} as a SIMD value", type_name(value)))
    })?;
    if target == Dtype::Bool {
        let low_bit = match &lanes {
            SimdLanes::Int(v) => v.iter().map(|x| x & 1 != 0).collect(),
            SimdLanes::Float(v) => v.iter().map(|x| (x.trunc() as i128) & 1 != 0).collect(),
            SimdLanes::Bool(v) => v.clone(),
        };
        return Ok(simd_value(target, SimdLanes::Bool(low_bit)));
    }
    let cast = match (&lanes, target.is_float()) {
        (SimdLanes::Int(v), false) => {
            SimdLanes::Int(v.iter().map(|x| wrap_lane(target, *x)).collect())
        }
        (SimdLanes::Int(v), true) => {
            SimdLanes::Float(v.iter().map(|x| target.round_lane(*x as f64)).collect())
        }
        (SimdLanes::Float(v), false) => SimdLanes::Int(
            v.iter()
                .map(|x| wrap_lane(target, x.trunc() as i128))
                .collect(),
        ),
        (SimdLanes::Float(v), true) => {
            SimdLanes::Float(v.iter().map(|x| target.round_lane(*x)).collect())
        }
        (SimdLanes::Bool(v), false) => SimdLanes::Int(v.iter().map(|x| i128::from(*x)).collect()),
        (SimdLanes::Bool(v), true) => {
            SimdLanes::Float(v.iter().map(|x| f64::from(u8::from(*x))).collect())
        }
    };
    Ok(simd_value(target, cast))
}

/// Lane gather for `v.shuffle[*mask]()`, `v.slice[...]()`, and `v.join(w)`.
///
/// Result lane `i` is lane `mask[i]` of the source's lanes followed by
/// `other`'s. The checker bounded every index; the bounds check here is the
/// phase-boundary backstop.
pub fn simd_shuffle(
    value: &Value,
    other: Option<&Value>,
    mask: &[usize],
) -> Result<Value, RuntimeError> {
    let lanes_of = |value: &Value| {
        simd_parts(value).ok_or_else(|| {
            RuntimeError::TypeError(format!(
                "cannot shuffle {} as a SIMD value",
                type_name(value)
            ))
        })
    };
    let (dtype, mut lanes) = lanes_of(value)?;
    if let Some(other) = other {
        lanes = match (lanes, lanes_of(other)?.1) {
            (SimdLanes::Int(mut head), SimdLanes::Int(tail)) => {
                head.extend(tail);
                SimdLanes::Int(head)
            }
            (SimdLanes::Float(mut head), SimdLanes::Float(tail)) => {
                head.extend(tail);
                SimdLanes::Float(head)
            }
            (SimdLanes::Bool(mut head), SimdLanes::Bool(tail)) => {
                head.extend(tail);
                SimdLanes::Bool(head)
            }
            _ => {
                return Err(RuntimeError::TypeError(
                    "cannot join SIMD values of different lane kinds".to_string(),
                ));
            }
        };
    }
    let width = lanes.width();
    if let Some(bad) = mask.iter().find(|lane| **lane >= width) {
        return Err(RuntimeError::TypeError(format!(
            "shuffle lane {bad} is out of range for {width} source lanes"
        )));
    }
    let gathered = match &lanes {
        SimdLanes::Int(v) => SimdLanes::Int(mask.iter().map(|lane| v[*lane]).collect()),
        SimdLanes::Float(v) => SimdLanes::Float(mask.iter().map(|lane| v[*lane]).collect()),
        SimdLanes::Bool(v) => SimdLanes::Bool(mask.iter().map(|lane| v[*lane]).collect()),
    };
    Ok(simd_value(dtype, gathered))
}

/// Dispatch a method call on a SIMD receiver: the numeric lane
/// reductions. The checker validated shapes; an unknown name is a clean
/// has-no-method error.
pub fn simd_method(
    dtype: Dtype,
    lanes: &SimdLanes,
    method: &str,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    match method {
        "reduce_add" | "reduce_mul" | "reduce_min" | "reduce_max" if args.is_empty() => {
            simd_reduce(dtype, lanes, method)
        }
        _ => Err(RuntimeError::Unsupported(format!(
            "vm: SIMD[DType.{}, {}] has no method '{method}'",
            dtype.name(),
            lanes.width()
        ))),
    }
}

/// `pop.select`: `when_true` where the one-lane `bool` `condition` is set,
/// else `when_false`. The cases are trivially register-passable, so the
/// chosen one is a bit copy.
pub fn select(
    condition: &Value,
    when_true: &Value,
    when_false: &Value,
) -> Result<Value, RuntimeError> {
    match simd_parts(condition) {
        Some((_, SimdLanes::Bool(lanes))) if lanes.len() == 1 => Ok(if lanes[0] {
            when_true.clone()
        } else {
            when_false.clone()
        }),
        _ => Err(RuntimeError::TypeError(
            "select requires a one-lane bool condition".to_string(),
        )),
    }
}

/// `simd.select`: lane `i` of the result takes `when_true`'s lane where
/// lane `i` of the bool `mask` is set, else `when_false`'s.
pub fn simd_select(
    mask: &Value,
    when_true: &Value,
    when_false: &Value,
) -> Result<Value, RuntimeError> {
    let Some((_, SimdLanes::Bool(mask))) = simd_parts(mask) else {
        return Err(RuntimeError::TypeError(
            "simd.select requires a bool mask".to_string(),
        ));
    };
    let width = mask.len();
    let Some((dtype, _)) = simd_parts(when_true) else {
        return Err(RuntimeError::TypeError(
            "simd.select requires SIMD cases".to_string(),
        ));
    };
    let lanes = if dtype == Dtype::Bool {
        let t = to_bool_lanes(when_true, width)?;
        let f = to_bool_lanes(when_false, width)?;
        SimdLanes::Bool(
            mask.iter()
                .enumerate()
                .map(|(i, m)| if *m { t[i] } else { f[i] })
                .collect(),
        )
    } else if dtype.is_float() {
        let t = to_float_lanes(when_true, dtype, width)?;
        let f = to_float_lanes(when_false, dtype, width)?;
        SimdLanes::Float(
            mask.iter()
                .enumerate()
                .map(|(i, m)| if *m { t[i] } else { f[i] })
                .collect(),
        )
    } else {
        let t = to_int_lanes(when_true, dtype, width)?;
        let f = to_int_lanes(when_false, dtype, width)?;
        SimdLanes::Int(
            mask.iter()
                .enumerate()
                .map(|(i, m)| if *m { t[i] } else { f[i] })
                .collect(),
        )
    };
    Ok(simd_value(dtype, lanes))
}

/// `simd.reduce`: the bitwise and or or of the integer or `bool` lanes of
/// `value`, one lane of its dtype.
pub fn simd_reduce_lanes(
    op: mojito_types::mlir::SimdReduction,
    value: &Value,
) -> Result<Value, RuntimeError> {
    // A one-lane mask (a width-one comparison's result) is held as `Bool`.
    let parts = match value {
        Value::Bool(lane) => Some((Dtype::Bool, SimdLanes::Bool(vec![*lane]))),
        other => simd_parts(other),
    };
    let lane = match parts {
        Some((dtype, SimdLanes::Bool(lanes))) => (
            dtype,
            SimdLanes::Bool(vec![op.reduce_bools(lanes.into_iter())]),
        ),
        Some((dtype, SimdLanes::Int(lanes))) => (
            dtype,
            SimdLanes::Int(vec![op.reduce_ints(lanes.into_iter())]),
        ),
        _ => {
            return Err(RuntimeError::TypeError(format!(
                "simd.reduce {} requires integer or bool lanes",
                op.mnemonic()
            )));
        }
    };
    Ok(simd_value(lane.0, lane.1))
}

/// Collapse lanes to the canonicalized width-1 scalar. Integer `add`/`mul`
/// wrap at the element width per step, like the elementwise operators;
/// `float32` rounds each step to single precision.
fn simd_reduce(dtype: Dtype, lanes: &SimdLanes, method: &str) -> Result<Value, RuntimeError> {
    match lanes {
        SimdLanes::Int(v) => {
            let mut acc = v[0];
            for x in &v[1..] {
                acc = match method {
                    "reduce_add" => wrap_lane(dtype, acc.wrapping_add(*x)),
                    "reduce_mul" => wrap_lane(dtype, acc.wrapping_mul(*x)),
                    "reduce_min" => acc.min(*x),
                    _ => acc.max(*x),
                };
            }
            Ok(simd_value(dtype, SimdLanes::Int(vec![acc])))
        }
        SimdLanes::Float(v) => {
            let mut acc = v[0];
            for x in &v[1..] {
                acc = match method {
                    "reduce_add" => dtype.round_lane(acc + x),
                    "reduce_mul" => dtype.round_lane(acc * x),
                    "reduce_min" => acc.min(*x),
                    _ => acc.max(*x),
                };
            }
            Ok(simd_value(dtype, SimdLanes::Float(vec![acc])))
        }
        SimdLanes::Bool(_) => Err(RuntimeError::TypeError(format!(
            "{method}() requires numeric lanes"
        ))),
    }
}

/// Materialize an operand as `width` integer lanes of `dtype` — its own lanes if
/// it is a SIMD of that width, else the scalar (or width-1 SIMD) splatted
/// across all lanes.
fn to_int_lanes(v: &Value, dtype: Dtype, width: usize) -> Result<Vec<i128>, RuntimeError> {
    match v {
        Value::Simd {
            lanes: SimdLanes::Int(l),
            ..
        } if l.len() == width => Ok(l.clone()),
        scalar => Ok(vec![value_to_int_lane(scalar, dtype)?; width]),
    }
}

fn to_float_lanes(v: &Value, dtype: Dtype, width: usize) -> Result<Vec<f64>, RuntimeError> {
    match v {
        Value::Simd {
            lanes: SimdLanes::Float(l),
            ..
        } if l.len() == width => Ok(l.clone()),
        scalar => Ok(vec![value_to_float_lane(scalar, dtype)?; width]),
    }
}

fn to_bool_lanes(v: &Value, width: usize) -> Result<Vec<bool>, RuntimeError> {
    match v {
        Value::Simd {
            lanes: SimdLanes::Bool(l),
            ..
        } if l.len() == width => Ok(l.clone()),
        scalar => Ok(vec![value_to_bool_lane(scalar)?; width]),
    }
}

fn exact_numeric_op(op: InfixOp, left: ExactNum, right: ExactNum) -> Result<Value, RuntimeError> {
    use InfixOp::{
        Add, And, BitAnd, BitOr, BitXor, Div, Eq, FloorDiv, Ge, Gt, In, Is, IsNot, Le, Lt, MatMul,
        Mod, Mul, Ne, NotIn, Or, Pow, Shl, Shr, Sub,
    };
    if let (ExactNum::Int(left), ExactNum::Int(right)) = (&left, &right) {
        let value = match op {
            Add => Value::IntLiteral(left.add(right)),
            Sub => Value::IntLiteral(left.sub(right)),
            Mul => Value::IntLiteral(left.mul(right)),
            // One condition, one message: the exact-literal operands
            // reach the same zero divisor the runtime `Int` path does
            // (`int_floor_div`/`int_floor_mod`), and the native backend
            // traps it under this text. A second spelling here only made
            // the two disagree.
            Div => Value::FloatLiteral(
                mojito_common::literal::FloatLiteral::from_int(left)
                    .div(&mojito_common::literal::FloatLiteral::from_int(right))
                    .ok_or_else(|| {
                        RuntimeError::TypeError("integer division or modulo by zero".to_string())
                    })?,
            ),
            FloorDiv => Value::IntLiteral(left.floor_div(right).ok_or_else(|| {
                RuntimeError::TypeError("integer division or modulo by zero".to_string())
            })?),
            Mod => Value::IntLiteral(left.floor_mod(right).ok_or_else(|| {
                RuntimeError::TypeError("integer division or modulo by zero".to_string())
            })?),
            Pow => Value::IntLiteral(left.pow(right).ok_or_else(|| {
                RuntimeError::TypeError(
                    "integer literal exponent must be a non-negative u32".to_string(),
                )
            })?),
            Shl => Value::IntLiteral(left.shl(right).ok_or_else(|| {
                RuntimeError::TypeError("invalid integer literal shift".to_string())
            })?),
            Shr => Value::IntLiteral(left.shr(right).ok_or_else(|| {
                RuntimeError::TypeError("invalid integer literal shift".to_string())
            })?),
            BitAnd => Value::IntLiteral(left.bitand(right)),
            BitOr => Value::IntLiteral(left.bitor(right)),
            BitXor => Value::IntLiteral(left.bitxor(right)),
            Lt | Le | Gt | Ge | Eq | Ne => {
                return Ok(Value::Bool(match op {
                    Lt => left < right,
                    Le => left <= right,
                    Gt => left > right,
                    Ge => left >= right,
                    Eq => left == right,
                    Ne => left != right,
                    _ => unreachable!(),
                }));
            }
            MatMul | And | Or | In | NotIn | Is | IsNot => {
                return Err(RuntimeError::TypeError(format!(
                    "operator '{op:?}' is invalid for IntLiteral"
                )));
            }
        };
        return Ok(value);
    }

    let (left, right) = (left.into_float(), right.into_float());
    Ok(match op {
        Add => Value::FloatLiteral(left.add(&right)),
        Sub => Value::FloatLiteral(left.sub(&right)),
        Mul => Value::FloatLiteral(left.mul(&right)),
        Div => Value::FloatLiteral(left.div(&right).ok_or_else(|| {
            RuntimeError::TypeError("floating literal division by zero".to_string())
        })?),
        FloorDiv => Value::FloatLiteral(left.floor_div(&right).ok_or_else(|| {
            RuntimeError::TypeError("floating literal division by zero".to_string())
        })?),
        Mod => Value::FloatLiteral(left.floor_mod(&right).ok_or_else(|| {
            RuntimeError::TypeError("floating literal modulo by zero".to_string())
        })?),
        Pow => {
            if let Some(exponent) = right.to_int_if_whole() {
                Value::FloatLiteral(left.pow_int(&exponent).ok_or_else(|| {
                    RuntimeError::TypeError("invalid FloatLiteral exponent".to_string())
                })?)
            } else {
                Value::Float64(
                    left.to_f64()
                        .ok_or_else(|| literal_materialization_error(&left, "Float64"))?
                        .powf(
                            right
                                .to_f64()
                                .ok_or_else(|| literal_materialization_error(&right, "Float64"))?,
                        ),
                )
            }
        }
        Lt | Le | Gt | Ge | Eq | Ne => Value::Bool(match op {
            Lt => left.numeric_cmp(&right).is_lt(),
            Le => !left.numeric_cmp(&right).is_gt(),
            Gt => left.numeric_cmp(&right).is_gt(),
            Ge => !left.numeric_cmp(&right).is_lt(),
            Eq => left.numeric_cmp(&right).is_eq(),
            Ne => !left.numeric_cmp(&right).is_eq(),
            _ => unreachable!(),
        }),
        Shl | Shr | BitAnd | BitOr | BitXor | MatMul | And | Or | In | NotIn | Is | IsNot => {
            return Err(RuntimeError::TypeError(format!(
                "operator '{op:?}' is invalid for FloatLiteral"
            )));
        }
    })
}

/// Apply a numeric binary operator, promoting operands to a common kind. The
/// checker rejects mixing two *concrete* numeric types, so any kind difference
/// here comes from a materialized literal — promotion resolves it soundly.
fn numeric_op(op: InfixOp, a: Num, b: Num) -> Result<Value, RuntimeError> {
    match a.rank().max(b.rank()) {
        2 => float_op(op, a.as_f64(), b.as_f64()),
        1 => uint_op(op, a.as_u64(), b.as_u64()),
        _ => int_op(op, a.as_i64(), b.as_i64()),
    }
}

fn int_op(op: InfixOp, x: i64, y: i64) -> Result<Value, RuntimeError> {
    use InfixOp::{
        Add, And, BitAnd, BitOr, BitXor, Div, Eq, FloorDiv, Ge, Gt, In, Is, IsNot, Le, Lt, MatMul,
        Mod, Mul, Ne, NotIn, Or, Pow, Shl, Shr, Sub,
    };
    Ok(match op {
        // Int overflow is defined two's-complement wrapping (the native ABI
        // contract, docs/native-abi.md); `//`, `%`, and `**` keep their
        // checked zero-divisor/exponent traps.
        Add => Value::Int(x.wrapping_add(y)),
        Sub => Value::Int(x.wrapping_sub(y)),
        Mul => Value::Int(x.wrapping_mul(y)),
        // Upstream's `pop.div`: truncating, and `MIN / -1` wraps.
        Div => Value::Int(x.wrapping_div(nonzero(y)?)),
        FloorDiv => Value::Int(floor_div(x, nonzero(y)?)),
        Mod => Value::Int(floor_mod(x, nonzero(y)?)),
        Shl => Value::Int(x.wrapping_shl(y as u32)),
        Shr => Value::Int(x.wrapping_shr(y as u32)),
        BitAnd => Value::Int(x & y),
        BitOr => Value::Int(x | y),
        BitXor => Value::Int(x ^ y),
        Lt => Value::Bool(x < y),
        Gt => Value::Bool(x > y),
        Le => Value::Bool(x <= y),
        Ge => Value::Bool(x >= y),
        Eq => Value::Bool(x == y),
        Ne => Value::Bool(x != y),
        // `**` runs the bundled `std._intrinsics._pow_int` body, dispatched by
        // `VmBackend::integer_pow` before an operand pair reaches here.
        Pow | MatMul | And | Or | In | NotIn | Is | IsNot => {
            return Err(RuntimeError::TypeError(format!(
                "operator '{op:?}' is invalid for integer dispatch"
            )));
        }
    })
}

fn uint_op(op: InfixOp, x: u64, y: u64) -> Result<Value, RuntimeError> {
    use InfixOp::{
        Add, And, BitAnd, BitOr, BitXor, Div, Eq, FloorDiv, Ge, Gt, In, Is, IsNot, Le, Lt, MatMul,
        Mod, Mul, Ne, NotIn, Or, Pow, Shl, Shr, Sub,
    };
    Ok(match op {
        // UInt overflow wraps mod 2^64 (the native ABI contract).
        Add => Value::UInt(x.wrapping_add(y)),
        Sub => Value::UInt(x.wrapping_sub(y)),
        Mul => Value::UInt(x.wrapping_mul(y)),
        // Unsigned: true and floor division and modulo are plain `/` and `%`.
        Div | FloorDiv => Value::UInt(x / nonzero_u(y)?),
        Mod => Value::UInt(x % nonzero_u(y)?),
        Shl => Value::UInt(x.wrapping_shl(y as u32)),
        Shr => Value::UInt(x.wrapping_shr(y as u32)),
        BitAnd => Value::UInt(x & y),
        BitOr => Value::UInt(x | y),
        BitXor => Value::UInt(x ^ y),
        Lt => Value::Bool(x < y),
        Gt => Value::Bool(x > y),
        Le => Value::Bool(x <= y),
        Ge => Value::Bool(x >= y),
        Eq => Value::Bool(x == y),
        Ne => Value::Bool(x != y),
        // `**` runs the bundled `std._intrinsics._pow_int` body, dispatched by
        // `VmBackend::integer_pow` before an operand pair reaches here.
        Pow | MatMul | And | Or | In | NotIn | Is | IsNot => {
            return Err(RuntimeError::TypeError(format!(
                "operator '{op:?}' is invalid for unsigned dispatch"
            )));
        }
    })
}

#[allow(
    clippy::float_cmp,
    reason = "exact comparison is the semantics of Mojo `==` on Float64"
)]
fn float_op(op: InfixOp, x: f64, y: f64) -> Result<Value, RuntimeError> {
    use InfixOp::{
        Add, And, BitAnd, BitOr, BitXor, Div, Eq, FloorDiv, Ge, Gt, In, Is, IsNot, Le, Lt, MatMul,
        Mod, Mul, Ne, NotIn, Or, Pow, Shl, Shr, Sub,
    };
    Ok(match op {
        Add => Value::Float64(x + y),
        Sub => Value::Float64(x - y),
        Mul => Value::Float64(x * y),
        Div => Value::Float64(x / y),
        FloorDiv => Value::Float64((x / y).floor()),
        Mod => Value::Float64(y.mul_add(-(x / y).floor(), x)),
        Pow => Value::Float64(x.powf(y)),
        Lt => Value::Bool(x < y),
        Gt => Value::Bool(x > y),
        Le => Value::Bool(x <= y),
        Ge => Value::Bool(x >= y),
        Eq => Value::Bool(x == y),
        // `pop.cmp ne` is the ordered predicate: a NaN is unequal to nothing.
        Ne => Value::Bool(x.partial_cmp(&y).is_some_and(std::cmp::Ordering::is_ne)),
        MatMul | Shl | Shr | BitAnd | BitOr | BitXor | And | Or | In | NotIn | Is | IsNot => {
            return Err(RuntimeError::TypeError(format!(
                "operator '{op:?}' is invalid for float dispatch"
            )));
        }
    })
}

/// A scalar value's integer content, for building an integer SIMD lane.
fn value_to_int(v: &Value) -> Result<i128, RuntimeError> {
    match v {
        Value::Int(n) => Ok(*n as i128),
        Value::IntLiteral(value) => value
            .to_i128()
            .ok_or_else(|| literal_materialization_error(value, "integer SIMD lane")),
        Value::UInt(n) => Ok(*n as i128),
        Value::Bool(b) => Ok(*b as i128),
        Value::Simd {
            lanes: SimdLanes::Int(l),
            ..
        } if l.len() == 1 => Ok(l[0]),
        other => Err(RuntimeError::TypeError(format!(
            "cannot use {} as an integer SIMD element",
            type_name(other)
        ))),
    }
}

// --- Collections / indexing ---

/// Convert a signed index into a bounds-checked `usize`, or a runtime error.
pub fn bounds_check(idx: i64, len: usize, what: &str) -> Result<usize, RuntimeError> {
    if idx < 0 || idx as usize >= len {
        return Err(RuntimeError::TypeError(format!(
            "{what} index {idx} out of range 0..{len}"
        )));
    }
    Ok(idx as usize)
}

// --- SIMD ---

// --- Shared numeric/utility built-ins (value level) -------------------------
//
// These operate on already-evaluated `Value`s so ordinary VM execution and
// VM-backed CTFE apply identical semantics. The checker guarantees arity and
// typing; these functions are the runtime realization only.

/// `abs(x)`: absolute value of a numeric (`UInt` is already non-negative).
pub fn builtin_abs(v: Value) -> Result<Value, RuntimeError> {
    match v {
        Value::Int(n) => Ok(Value::Int(n.wrapping_abs())),
        Value::Float64(x) => Ok(Value::Float64(x.abs())),
        Value::IntLiteral(value) => Ok(Value::IntLiteral(if value.is_negative() {
            value.neg()
        } else {
            value
        })),
        Value::FloatLiteral(value) => Ok(Value::FloatLiteral(value.abs())),
        Value::UInt(n) => Ok(Value::UInt(n)),
        other => Err(RuntimeError::TypeError(format!(
            "abs() expects a numeric value, got {}",
            type_name(&other)
        ))),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeError {
    UndefinedVariable(String),
    TypeError(String),
    NotCallable(String),
    ArityMismatch {
        name: String,
        expected: usize,
        got: usize,
    },
    /// A closure value tried to escape its defining scope (e.g. by being
    /// returned). Mojo does not support escaping closures — downward funargs
    /// only. See the strict-subset design notes.
    ClosureEscape,
    /// An error raised by `raise` that was not caught by any `try`/`except`. It
    /// propagates through VM execution and is reported if it reaches the top.
    Raised(crate::runtime::Value),
    /// A valid-Mojo construct that reaches a compiler/backend boundary whose
    /// semantics Mojito does not implement. Carries a feature description.
    Unsupported(String),
    /// An uncatchable trap: `os.abort` under the CPU-default assertion
    /// configuration (strict slice bounds, explicit assertion failures).
    /// Unlike `Raised`, no `try`/`except` observes it; it always propagates
    /// to the top and ends the program.
    Abort(String),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UndefinedVariable(name) => write!(f, "Undefined variable '{name}'"),
            Self::TypeError(msg) => write!(f, "Type error: {msg}"),
            Self::NotCallable(name) => write!(f, "'{name}' is not callable"),
            Self::ArityMismatch {
                name,
                expected,
                got,
            } => {
                write!(f, "'{name}' expects {expected} argument(s), got {got}")
            }
            Self::ClosureEscape => {
                write!(
                    f,
                    "closures cannot escape their defining scope (downward funargs only)"
                )
            }
            Self::Raised(value) => write!(f, "unhandled error: {value}"),
            Self::Unsupported(what) => write!(f, "unsupported feature: {what}"),
            Self::Abort(message) => write!(f, "abort: {message}"),
        }
    }
}

impl std::error::Error for RuntimeError {}

/// A mask of the low `bits` bits of a `u64`.
const fn low_bits(bits: usize) -> u64 {
    if bits >= 64 {
        u64::MAX
    } else {
        (1 << bits) - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mojito_common::literal::FloatLiteral;

    /// Int/UInt overflow is defined two's-complement wrapping (the native ABI
    /// contract, docs/native-abi.md) — including the single overflowing
    /// signed-division case and wrapping `**`.
    #[test]
    fn integer_overflow_wraps_by_definition() {
        let int = |value| Value::Int(value);
        assert_eq!(int_op(InfixOp::Add, i64::MAX, 1).unwrap(), int(i64::MIN));
        assert_eq!(int_op(InfixOp::Sub, i64::MIN, 1).unwrap(), int(i64::MAX));
        assert_eq!(int_op(InfixOp::Mul, i64::MAX, 2).unwrap(), int(-2));
        assert_eq!(
            int_op(InfixOp::FloorDiv, i64::MIN, -1).unwrap(),
            int(i64::MIN)
        );
        assert_eq!(int_op(InfixOp::Mod, i64::MIN, -1).unwrap(), int(0));
        // Zero divisors still trap; only overflow got defined. `**` wraps in
        // the bundled `_pow_int` body, so only its exponent guard is here.
        assert!(int_op(InfixOp::FloorDiv, 1, 0).is_err());
        assert!(pow_exponent(-1).is_err());
        assert!(pow_exponent(i64::from(u32::MAX)).is_ok());

        let uint = |value| Value::UInt(value);
        assert_eq!(uint_op(InfixOp::Sub, 0, 1).unwrap(), uint(u64::MAX));
        assert_eq!(uint_op(InfixOp::Add, u64::MAX, 1).unwrap(), uint(0));
        assert_eq!(
            uint_op(InfixOp::Mul, u64::MAX, 2).unwrap(),
            uint(u64::MAX - 1)
        );
        assert!(uint_op(InfixOp::Mod, 1, 0).is_err());
    }

    /// Sized integer lanes wrap like the scalars, and `//`/`%` follow
    /// upstream's integer `SIMD` rule: floor semantics with a zero divisor
    /// selecting a zero lane.
    #[test]
    fn simd_integer_lanes_wrap_and_floor_divide() {
        let lane = |dtype, value: i128| Value::Simd {
            dtype,
            lanes: SimdLanes::Int(vec![value]),
        };
        let u64_max = i128::from(u64::MAX);
        let product = simd_binop(
            InfixOp::Mul,
            &lane(Dtype::UInt64, u64_max),
            &lane(Dtype::UInt64, u64_max),
        )
        .unwrap();
        assert_eq!(product, lane(Dtype::UInt64, 1));
        let floor =
            |op, a, b| simd_binop(op, &lane(Dtype::Int64, a), &lane(Dtype::Int64, b)).unwrap();
        assert_eq!(floor(InfixOp::FloorDiv, -7, 2), lane(Dtype::Int64, -4));
        assert_eq!(floor(InfixOp::Mod, -7, 2), lane(Dtype::Int64, 1));
        assert_eq!(floor(InfixOp::FloorDiv, 7, -2), lane(Dtype::Int64, -4));
        assert_eq!(floor(InfixOp::Mod, 7, 0), lane(Dtype::Int64, 0));
        assert_eq!(floor(InfixOp::FloorDiv, 7, 0), lane(Dtype::Int64, 0));
        let unsigned = simd_binop(
            InfixOp::FloorDiv,
            &lane(Dtype::UInt64, u64_max),
            &lane(Dtype::UInt64, 3),
        )
        .unwrap();
        assert_eq!(unsigned, lane(Dtype::UInt64, u64_max / 3));
    }

    #[test]
    fn exact_signed_zero_uses_numeric_equality_and_abs() {
        let positive = Value::FloatLiteral(FloatLiteral::parse_decimal("0.0").unwrap());
        let negative = Value::FloatLiteral(FloatLiteral::parse_decimal("0.0").unwrap().neg());

        assert!(values_equal(&positive, &negative).unwrap());
        let Value::FloatLiteral(magnitude) = builtin_abs(negative).unwrap() else {
            panic!("literal abs changed the value kind");
        };
        assert_eq!(magnitude.to_f64().unwrap().to_bits(), 0.0f64.to_bits());
    }

    #[test]
    fn simd_bitcast_keeps_the_bits_and_folds_nothing() {
        let simd = |dtype, lanes| Value::Simd { dtype, lanes };
        let ints = |dtype, lanes: &[i128]| simd(dtype, SimdLanes::Int(lanes.to_vec()));
        let bools = SimdLanes::Bool(vec![true, true, false, true, false, false, false, false]);
        assert_eq!(
            simd_bitcast(Dtype::UInt8, 1, &simd(Dtype::Bool, bools.clone())).unwrap(),
            ints(Dtype::UInt8, &[11])
        );
        assert_eq!(
            simd_bitcast(Dtype::Bool, 8, &ints(Dtype::UInt8, &[11])).unwrap(),
            simd(Dtype::Bool, bools)
        );
        assert_eq!(
            simd_bitcast(
                Dtype::UInt8,
                2,
                &simd(Dtype::Bool, SimdLanes::Bool(vec![true; 16]))
            )
            .unwrap(),
            ints(Dtype::UInt8, &[255, 255])
        );
        assert_eq!(
            simd_bitcast(
                Dtype::Int16,
                4,
                &simd(Dtype::Float32, SimdLanes::Float(vec![1.0, -2.0]))
            )
            .unwrap(),
            ints(Dtype::Int16, &[0, 16256, 0, -16384])
        );
        assert!(matches!(
            simd_bitcast(Dtype::Float64, 1, &Value::Int(-1)).unwrap(),
            Value::Float64(x) if x.is_nan()
        ));
        let negative_zero = simd_bitcast(Dtype::UInt64, 1, &Value::Float64(-0.0)).unwrap();
        assert_eq!(
            negative_zero,
            ints(Dtype::UInt64, &[i128::from(1_u64 << 63)])
        );
        assert!(matches!(
            simd_bitcast(Dtype::Float64, 1, &negative_zero).unwrap(),
            Value::Float64(x) if x == 0.0 && x.is_sign_negative()
        ));
        let bytes = simd_bitcast(Dtype::UInt8, 4, &ints(Dtype::UInt32, &[4631])).unwrap();
        assert_eq!(bytes, ints(Dtype::UInt8, &[23, 18, 0, 0]));
        assert_eq!(
            simd_bitcast(Dtype::UInt32, 1, &bytes).unwrap(),
            ints(Dtype::UInt32, &[4631])
        );
        assert_eq!(hash_leaf_ty(&Value::Bool(true)), Some(Ty::bool()));
        assert_eq!(
            hash_leaf_value(Value::Dtype(Dtype::Int8)),
            ints(Dtype::UInt8, &[i128::from(Dtype::Int8.code())])
        );
        assert_eq!(hash_leaf_ty(&Value::Int(1)), None);
    }
}

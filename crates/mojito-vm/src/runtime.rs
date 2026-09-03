//! Runtime values and their operations, used by the register-VM `backend`. This
//! module owns the `Value` type (`SimdLanes` and all), scalar/`SIMD` operations,
//! coercion, and the utility built-ins — the shared value layer the
//! backend consumes.

use std::cmp::Ordering;
use std::fmt;
use std::io::{self, Write};

use mojito_ast::ast::{Dtype, InfixOp, PrefixOp, Type};

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
    /// A checked slice descriptor. Unlike the old fabricated struct layout,
    /// omitted bounds and descriptor kind are first-class runtime data.
    Slice {
        kind: mojito_types::types::SliceKind,
        start: Option<i64>,
        end: Option<i64>,
        step: Option<i64>,
    },
    /// A struct instance: its type name, field values (in declaration order), and
    /// any reified value parameters (`FixedBuffer[8]` stores `size = 8`, read via
    /// `Self.size` in methods). A value type — `Clone` deep-copies, giving value
    /// semantics.
    Struct {
        name: String,
        fields: Vec<(String, Value)>,
        value_params: Vec<(String, Value)>,
    },
    /// A SIMD vector: its element type and lane values. The width-1 case is a
    /// scalar-alias value (`Int32`, …).
    Simd {
        dtype: Dtype,
        lanes: SimdLanes,
    },
    /// An `Error` value carrying its message (what `raise` raises).
    Error(String),
    /// A list-shaped compile-time value crossing the narrow VM-CTFE bridge.
    /// This is never a language-level runtime collection: executable List values
    /// are ordinary nominal [`Value::Struct`] instances from the bundled library.
    ComptimeList(Vec<Value>),
    /// Internal heterogeneous pack storage shared by MIR and VM-CTFE. Public
    /// `Tuple` values are nominal structs and must not use this representation.
    Tuple(Vec<Value>),
    /// A tagged union. `alternatives` is the checked type-level ordering and
    /// `index` selects the active payload.
    Variant {
        alternatives: Vec<mojito_types::types::Ty>,
        index: usize,
        value: Box<Value>,
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
    UninitStorage(Option<Box<Value>>),
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

/// One slot in a lifted closure's environment. Reference captures retain their
/// original frame handle; copy/move captures own the declaration-time value and
/// are subsequently borrowed in place on every invocation.
#[derive(Debug, Clone)]
pub struct ClosureCapture {
    pub value: Value,
    pub owned: bool,
}

/// The lanes of a SIMD value, one representation per element-type kind. Integer
/// lanes are stored as `i128` holding the post-wrap mathematical value (so an
/// unsigned lane is its non-negative value), which makes comparisons correct for
/// both signed and unsigned dtypes.
#[derive(Debug, Clone, PartialEq)]
pub enum SimdLanes {
    Int(Vec<i128>),
    Float(Vec<f64>),
    Bool(Vec<bool>),
}

impl SimdLanes {
    fn width(&self) -> usize {
        match self {
            SimdLanes::Int(v) => v.len(),
            SimdLanes::Float(v) => v.len(),
            SimdLanes::Bool(v) => v.len(),
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::UInt(a), Value::UInt(b)) => a == b,
            (Value::Float64(a), Value::Float64(b)) => a == b,
            (Value::IntLiteral(a), Value::IntLiteral(b)) => a == b,
            (Value::FloatLiteral(a), Value::FloatLiteral(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (
                Value::Pointer {
                    allocation: aa,
                    offset: ao,
                },
                Value::Pointer {
                    allocation: ba,
                    offset: bo,
                },
            ) => aa == ba && ao == bo,
            (Value::None, Value::None) => true,
            // Closures have identity, not structural, equality.
            (
                Value::Slice {
                    kind: ak,
                    start: as_,
                    end: ae,
                    step: ap,
                },
                Value::Slice {
                    kind: bk,
                    start: bs,
                    end: be,
                    step: bp,
                },
            ) => ak == bk && as_ == bs && ae == be && ap == bp,
            (
                Value::Struct {
                    name: n1,
                    fields: f1,
                    value_params: p1,
                },
                Value::Struct {
                    name: n2,
                    fields: f2,
                    value_params: p2,
                },
            ) => n1 == n2 && f1 == f2 && p1 == p2,
            (
                Value::Simd {
                    dtype: d1,
                    lanes: l1,
                },
                Value::Simd {
                    dtype: d2,
                    lanes: l2,
                },
            ) => d1 == d2 && l1 == l2,
            (Value::Error(a), Value::Error(b)) => a == b,
            (Value::ComptimeList(a), Value::ComptimeList(b)) => a == b,
            (Value::Tuple(a), Value::Tuple(b)) => a == b,
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
            ) => aa == ba && ai == bi && av == bv,
            (
                Value::Ref {
                    frame: af,
                    slot: as_,
                    projection: ap,
                },
                Value::Ref {
                    frame: bf,
                    slot: bs,
                    projection: bp,
                },
            ) => af == bf && as_ == bs && ap == bp,
            (Value::UninitStorage(a), Value::UninitStorage(b)) => a == b,
            _ => false,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(n) => write!(f, "{}", n),
            Value::UInt(n) => write!(f, "{}", n),
            // `{:?}` keeps the decimal point (e.g. `3.0`), distinguishing from Int.
            Value::Float64(x) => write!(f, "{:?}", x),
            Value::IntLiteral(value) => write!(f, "{value}"),
            Value::FloatLiteral(value) => write!(f, "{value}"),
            Value::Bool(b) => write!(f, "{}", if *b { "True" } else { "False" }),
            Value::Str(s) => write!(f, "{}", s),
            Value::None => write!(f, "None"),
            Value::Function(name) => write!(f, "<function {name}>"),
            Value::Closure { function, .. } => write!(f, "<closure {function}>"),
            Value::Slice {
                kind,
                start,
                end,
                step,
            } => write!(
                f,
                "{}({:?}, {:?}, {:?})",
                kind.type_name(),
                start,
                end,
                step
            ),
            Value::Struct {
                name,
                fields,
                value_params,
            } => {
                write!(f, "{}", name)?;
                if !value_params.is_empty() {
                    write!(f, "[")?;
                    for (i, (_, val)) in value_params.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{}", val)?;
                    }
                    write!(f, "]")?;
                }
                write!(f, "(")?;
                for (i, (fname, val)) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}={}", fname, val)?;
                }
                write!(f, ")")
            }
            // A width-1 SIMD prints as the bare lane; wider as `[l0, l1, ...]`.
            Value::Simd { lanes, .. } => {
                let strs: Vec<String> = match lanes {
                    SimdLanes::Int(v) => v.iter().map(|n| n.to_string()).collect(),
                    SimdLanes::Float(v) => v.iter().map(|x| format!("{:?}", x)).collect(),
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
            Value::Error(msg) => write!(f, "{}", msg),
            Value::Pointer { allocation, offset } => {
                write!(f, "Pointer({allocation:#x}+{offset})")
            }
            Value::Ref { frame, slot, .. } => write!(f, "<ref {frame}:{slot}>"),
            Value::Moved => write!(f, "<moved>"),
            Value::UninitStorage(None) => write!(f, "<uninit>"),
            Value::UninitStorage(Some(value)) => write!(f, "<uninit-storage {value}>"),
            Value::ComptimeList(items) => {
                write!(f, "<comptime-list [")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", item)?;
                }
                write!(f, "]>")
            }
            Value::Tuple(items) => {
                write!(f, "(")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", item)?;
                }
                // A 1-tuple prints with a trailing comma, like Python/Mojo.
                if items.len() == 1 {
                    write!(f, ",")?;
                }
                write!(f, ")")
            }
            Value::Variant { value, .. } => value.fmt(f),
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
        Value::Str(_) => "String".to_string(),
        Value::None => "None".to_string(),
        Value::Function(_) | Value::Closure { .. } => "function".to_string(),
        Value::Slice { kind, .. } => kind.type_name().to_string(),
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
            if ai != bi {
                Ok(false)
            } else {
                values_equal(av, bv)
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

/// Normalize one scalar Hashable leaf to the UInt64 contribution consumed by
/// the hasher protocol.
pub fn hash_bits(value: &Value) -> Result<u64, RuntimeError> {
    match value {
        Value::Int(value) => Ok(*value as u64),
        Value::UInt(value) => Ok(*value),
        Value::Bool(value) => Ok(u64::from(*value)),
        Value::Float64(value) => Ok(if *value == 0.0 { 0 } else { value.to_bits() }),
        Value::IntLiteral(value) => value
            .wrapping_signed(64)
            .map(|value| value as u64)
            .ok_or_else(|| RuntimeError::TypeError("integer literal does not fit Int".into())),
        Value::Simd {
            dtype,
            lanes: SimdLanes::Int(values),
        } if values.len() == 1 => {
            let bits = integer_dtype_bits(*dtype).map_or(64, |(bits, _)| bits);
            let mask = if bits == 64 {
                u64::MAX
            } else {
                (1_u64 << bits) - 1
            };
            Ok((values[0] as u64) & mask)
        }
        Value::Simd {
            lanes: SimdLanes::Bool(values),
            ..
        } if values.len() == 1 => Ok(u64::from(values[0])),
        Value::Simd {
            dtype: Dtype::Float32,
            lanes: SimdLanes::Float(values),
        } if values.len() == 1 => {
            let value = values[0] as f32;
            Ok(u64::from(if value == 0.0 { 0 } else { value.to_bits() }))
        }
        other => Err(RuntimeError::TypeError(format!(
            "cannot hash {} as a scalar leaf",
            type_name(other)
        ))),
    }
}

/// Apply a unary operator to an already-evaluated operand for VM execution and
/// VM-backed compile-time evaluation.
pub fn apply_prefix(op: PrefixOp, value: Value) -> Result<Value, RuntimeError> {
    match (op, value) {
        (PrefixOp::Neg, Value::Int(n)) => Ok(Value::Int(n.wrapping_neg())),
        (PrefixOp::Neg, Value::Float64(x)) => Ok(Value::Float64(-x)),
        (PrefixOp::Neg, Value::IntLiteral(value)) => Ok(Value::IntLiteral(value.neg())),
        (PrefixOp::Neg, Value::FloatLiteral(value)) => Ok(Value::FloatLiteral(value.neg())),
        (PrefixOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
        // Elementwise negation wraps integer lanes at the element width;
        // float lanes negate exactly (a sign flip never re-rounds).
        (PrefixOp::Neg, Value::Simd { dtype, lanes }) => match lanes {
            SimdLanes::Int(v) => Ok(simd_value(
                dtype,
                SimdLanes::Int(
                    v.into_iter()
                        .map(|x| wrap(dtype, x.wrapping_neg()))
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
        (PrefixOp::Neg, v) => Err(RuntimeError::TypeError(format!(
            "cannot negate {}",
            type_name(&v)
        ))),
        (PrefixOp::Not, v) => Err(RuntimeError::TypeError(format!(
            "'not' requires Bool, got {}",
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
        return Ok(Value::Str(format!("{}{}", a, b)));
    }
    if let (Value::Tuple(left), Value::Tuple(right)) = (&l, &r) {
        let result = match op {
            InfixOp::Eq => Some(values_equal(&l, &r)?),
            InfixOp::Ne => Some(!values_equal(&l, &r)?),
            InfixOp::Lt => Some(compare_tuples(left, right)? == Ordering::Less),
            InfixOp::Le => Some(compare_tuples(left, right)? != Ordering::Greater),
            InfixOp::Gt => Some(compare_tuples(left, right)? == Ordering::Greater),
            InfixOp::Ge => Some(compare_tuples(left, right)? != Ordering::Less),
            _ => None,
        };
        if let Some(result) = result {
            return Ok(Value::Bool(result));
        }
    }
    // Numeric operators (arithmetic, ordering, and numeric equality), with
    // promotion of mixed numeric kinds.
    if let (Some(a), Some(b)) = (as_num(&l), as_num(&r)) {
        return numeric_op(op, a, b);
    }
    // Equality between non-numeric values (Bool/String/None).
    match op {
        InfixOp::Eq => Ok(Value::Bool(values_equal(&l, &r)?)),
        InfixOp::Ne => Ok(Value::Bool(!values_equal(&l, &r)?)),
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

/// Normalize a `Slice` against a concrete length, implementing the public
/// `Slice.indices(length)` contract.
pub fn normalize_slice_bounds(
    len: i64,
    lower: Option<i64>,
    upper: Option<i64>,
    step: Option<i64>,
) -> Result<(i64, i64, i64), RuntimeError> {
    if len < 0 {
        return Err(RuntimeError::TypeError(
            "slice length cannot be negative".to_string(),
        ));
    }
    let step = step.unwrap_or(1);
    if step == 0 {
        return Err(RuntimeError::TypeError(
            "slice step cannot be zero".to_string(),
        ));
    }
    // Clamp an explicit bound to a valid range, wrapping a negative index once.
    let adjust = |i: i64| -> i64 {
        let i = if i < 0 { i + len } else { i };
        if step > 0 {
            i.clamp(0, len)
        } else {
            i.clamp(-1, len - 1)
        }
    };
    let (start, stop) = if step > 0 {
        (lower.map_or(0, adjust), upper.map_or(len, adjust))
    } else {
        (lower.map_or(len - 1, adjust), upper.map_or(-1, adjust))
    };
    Ok((start, stop, step))
}

/// Evaluate primitive/internal `x in c` / `x not in c`. Nominal collections
/// dispatch through `__contains__`; this helper handles only internal pack
/// storage, the compile-time-only list bridge, and `String`.
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
    let (na, nb) = match (as_num(&a), as_num(&b)) {
        (Some(na), Some(nb)) => (na, nb),
        _ => {
            return Err(RuntimeError::TypeError(format!(
                "min()/max() expect numeric values, got {} and {}",
                type_name(&a),
                type_name(&b)
            )));
        }
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

/// `round(x)`: nearest `Float64` (ties away from zero, via `f64::round`).
pub fn builtin_round(v: Value) -> Result<Value, RuntimeError> {
    Ok(Value::Float64(value_to_float(&v)?.round()))
}

/// `input(prompt)`: write the prompt immediately, read one line from standard
/// input, and return it without the trailing line ending. EOF returns `""`, which
/// keeps noninteractive asset runs from blocking forever.
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
    io::stdin()
        .read_line(&mut line)
        .map_err(|e| RuntimeError::Unsupported(format!("input(): failed to read stdin: {e}")))?;
    if line.ends_with('\n') {
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
    }
    Ok(Value::Str(line))
}

/// `Int(x)` / `UInt(x)` / `Float64(x)` / `Bool(x)`: convert one numeric-or-`Bool`
/// value (`Float64`→integer truncates toward zero, `Bool` is 0/1, `Bool(x)` is
/// truthiness — a nonzero value is `True`), following Mojo.
pub fn builtin_convert(name: &str, v: Value) -> Result<Value, RuntimeError> {
    match &v {
        Value::IntLiteral(value) => {
            return Ok(match name {
                "Int" => Value::Int(
                    value
                        .wrapping_signed(64)
                        .ok_or_else(|| literal_materialization_error(&value, "Int"))?,
                ),
                "UInt" => Value::UInt(
                    value
                        .wrapping_unsigned(64)
                        .ok_or_else(|| literal_materialization_error(&value, "UInt"))?,
                ),
                "Bool" => Value::Bool(!value.is_zero()),
                _ => Value::Float64(
                    value
                        .to_f64()
                        .ok_or_else(|| literal_materialization_error(&value, "Float64"))?,
                ),
            });
        }
        Value::FloatLiteral(value) => {
            return Ok(match name {
                "Int" => Value::Int(
                    value
                        .trunc_to_int()
                        .wrapping_signed(64)
                        .ok_or_else(|| literal_materialization_error(&value, "Int"))?,
                ),
                "UInt" => Value::UInt(
                    value
                        .trunc_to_int()
                        .wrapping_unsigned(64)
                        .ok_or_else(|| literal_materialization_error(&value, "UInt"))?,
                ),
                "Bool" => Value::Bool(!value.is_zero()),
                _ => Value::Float64(
                    value
                        .to_f64()
                        .ok_or_else(|| literal_materialization_error(&value, "Float64"))?,
                ),
            });
        }
        _ => {}
    }
    let (as_i, as_u, as_f, as_bool) = match v {
        Value::Int(n) => (n, n as u64, n as f64, n != 0),
        Value::UInt(n) => (n as i64, n, n as f64, n != 0),
        Value::Float64(x) => (x as i64, x as u64, x, x != 0.0),
        Value::Bool(b) => (b as i64, b as u64, if b { 1.0 } else { 0.0 }, b),
        // A width-1 SIMD value is a scalar alias (`UInt8`, `Byte`, ...).
        Value::Simd { ref lanes, .. } if lanes.width() == 1 => match lanes {
            SimdLanes::Int(v) => (v[0] as i64, v[0] as u64, v[0] as f64, v[0] != 0),
            SimdLanes::Float(v) => (v[0] as i64, v[0] as u64, v[0], v[0] != 0.0),
            SimdLanes::Bool(v) => (
                i64::from(v[0]),
                u64::from(v[0]),
                if v[0] { 1.0 } else { 0.0 },
                v[0],
            ),
        },
        other => {
            return Err(RuntimeError::TypeError(format!(
                "{}() expects a numeric or Bool value, got {}",
                name,
                type_name(&other)
            )));
        }
    };
    Ok(match name {
        "Int" => Value::Int(as_i),
        "UInt" => Value::UInt(as_u),
        "Bool" => Value::Bool(as_bool),
        _ => Value::Float64(as_f), // "Float64"
    })
}

/// The intrinsic behind `math.floor`/`ceil`/`trunc` — i.e. the `Floorable`/
/// `Ceilable`/`Truncable` dunders on a built-in numeric value (roadmap milestone 7). An
/// integer is already whole, so it is returned unchanged; a `Float64` rounds
/// toward the requested direction, preserving its type (`__floor__(self) -> Self`).
pub fn builtin_round_dir(method: &str, v: &Value) -> Result<Value, RuntimeError> {
    match v {
        Value::Int(_) | Value::UInt(_) => Ok(v.clone()),
        Value::Float64(x) => Ok(Value::Float64(match method {
            "__floor__" => x.floor(),
            "__ceil__" => x.ceil(),
            _ => x.trunc(), // "__trunc__"
        })),
        other => Err(RuntimeError::TypeError(format!(
            "{}() expects a numeric value, got {}",
            method,
            type_name(other)
        ))),
    }
}

/// The intrinsic behind `math.ceildiv` — the `CeilDivable` dunder
/// (`__ceildiv__(self, denominator) -> Self`, roadmap milestone 7): ceiling division,
/// preserving the operand type. Integer ceildiv rounds toward +∞
/// (`ceildiv(-7, 2) == -3`); float ceildiv is `ceil(a / b)`.
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
/// in private pack storage using the operators' flooring rules. The VM wraps
/// this transient value in the checker-selected nominal public Tuple before it
/// crosses the instruction-result boundary.
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
/// Build a SIMD value of `dtype`/`width` from element `values`: exactly `width`
/// elements (one per lane) or a single element splatted to every lane.
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
/// element width exactly as construction does (`wrap`/`round_f32`).
pub fn set_simd_lane(
    dtype: Dtype,
    lanes: &mut SimdLanes,
    i: i64,
    value: Value,
) -> Result<(), RuntimeError> {
    let idx = bounds_check(i, lanes.width(), "SIMD lane")?;
    match lanes {
        SimdLanes::Int(v) => v[idx] = value_to_int_lane(&value, dtype)?,
        SimdLanes::Float(v) => v[idx] = value_to_float_lane(&value, dtype)?,
        SimdLanes::Bool(v) => v[idx] = value_to_bool_lane(&value)?,
    }
    Ok(())
}

/// Apply an elementwise SIMD operator. One operand may be a scalar that splats
/// to the other's dtype and width. Integer arithmetic is bit-accurate;
/// `float32` rounds each result to single precision. Comparisons yield a `bool`
/// mask. (The checker guarantees dtype/width agreement and operator validity.)
pub fn simd_binop(op: InfixOp, l: &Value, r: &Value) -> Result<Value, RuntimeError> {
    use InfixOp::*;
    let (dtype, width) = simd_shape(l).or_else(|| simd_shape(r)).ok_or_else(|| {
        RuntimeError::TypeError("SIMD dispatch requires at least one SIMD operand".to_string())
    })?;
    let is_cmp = matches!(op, Eq | Ne | Lt | Gt | Le | Ge);
    match dtype {
        Dtype::Bool => {
            let xs = to_bool_lanes(l, width)?;
            let ys = to_bool_lanes(r, width)?;
            let out: Vec<bool> = xs
                .iter()
                .zip(&ys)
                .map(|(a, b)| match op {
                    Eq => a == b,
                    Ne => a != b,
                    BitAnd => *a & *b,
                    BitOr => *a | *b,
                    BitXor => *a ^ *b,
                    _ => false, // checker rejects other ops on bool lanes
                })
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
                    .map(|(a, b)| float_cmp(op, *a, *b))
                    .collect();
                Ok(Value::Simd {
                    dtype: Dtype::Bool,
                    lanes: SimdLanes::Bool(out),
                })
            } else {
                let out: Vec<f64> = xs
                    .iter()
                    .zip(&ys)
                    .map(|(a, b)| round_lane(dtype, float_arith(op, *a, *b)))
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
                    .map(|(a, b)| int_cmp(op, *a, *b))
                    .collect();
                Ok(Value::Simd {
                    dtype: Dtype::Bool,
                    lanes: SimdLanes::Bool(out),
                })
            } else {
                let out: Vec<i128> = xs
                    .iter()
                    .zip(&ys)
                    .map(|(a, b)| wrap(d, int_arith(op, d, *a, *b)))
                    .collect();
                Ok(Value::Simd {
                    dtype: d,
                    lanes: SimdLanes::Int(out),
                })
            }
        }
    }
}

/// Coerce a value to a declared type at a binding site. Only materializes a
/// numeric literal's default (`Int`/`Float64`) into another numeric type; all
/// other values pass through unchanged.
pub fn coerce(value: Value, ty: &Type) -> Value {
    match ty {
        Type::UInt => match value {
            Value::Int(n) => Value::UInt(n as u64),
            Value::IntLiteral(value) => value
                .to_u64()
                .map(Value::UInt)
                .unwrap_or_else(|| Value::IntLiteral(value)),
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
    use mojito_types::types::Ty;
    match (value, target) {
        (Value::IntLiteral(value), Ty::Int) => value
            .wrapping_signed(64)
            .map(Value::Int)
            .ok_or_else(|| literal_materialization_error(&value, "Int")),
        (Value::IntLiteral(value), Ty::UInt) => value
            .wrapping_unsigned(64)
            .map(Value::UInt)
            .ok_or_else(|| literal_materialization_error(&value, "UInt")),
        (Value::IntLiteral(value), Ty::Float64) => value
            .to_f64()
            .map(Value::Float64)
            .ok_or_else(|| literal_materialization_error(&value, "Float64")),
        (Value::FloatLiteral(value), Ty::Float64) => value
            .to_f64()
            .map(Value::Float64)
            .ok_or_else(|| literal_materialization_error(&value, "Float64")),
        (Value::IntLiteral(value), Ty::Simd { dtype, width: 1 }) if dtype.is_float() => {
            let lane = if *dtype == Dtype::Float32 {
                mojito_common::literal::FloatLiteral::from_int(&value)
                    .to_f32()
                    .map(|value| value as f64)
            } else {
                value.to_f64()
            }
            .ok_or_else(|| literal_materialization_error(&value, &format!("{dtype:?}")))?;
            Ok(simd_value(*dtype, SimdLanes::Float(vec![lane])))
        }
        (Value::FloatLiteral(value), Ty::Simd { dtype, width: 1 }) if dtype.is_float() => {
            let lane = if *dtype == Dtype::Float32 {
                value.to_f32().map(|value| value as f64)
            } else {
                value.to_f64()
            }
            .ok_or_else(|| literal_materialization_error(&value, &format!("{dtype:?}")))?;
            Ok(simd_value(*dtype, SimdLanes::Float(vec![lane])))
        }
        (Value::IntLiteral(value), Ty::Simd { dtype, width: 1 }) => {
            let lane = materialize_literal_int_lane(&value, *dtype)
                .ok_or_else(|| literal_materialization_error(&value, &format!("{dtype:?}")))?;
            Ok(simd_value(*dtype, SimdLanes::Int(vec![lane])))
        }
        (value, target) => Err(RuntimeError::TypeError(format!(
            "cannot materialize {} as {target}",
            type_name(&value)
        ))),
    }
}

pub fn coerce_checked(value: Value, ty: &mojito_types::types::Ty) -> Value {
    use mojito_types::types::Ty;
    if matches!(value, Value::IntLiteral(_) | Value::FloatLiteral(_))
        && let Ok(materialized) = materialize_literal(value.clone(), ty)
    {
        return materialized;
    }
    match ty {
        Ty::UInt => match value {
            Value::Int(n) => Value::UInt(n as u64),
            value => value,
        },
        Ty::Float64 => match value {
            Value::Int(n) => Value::Float64(n as f64),
            Value::UInt(n) => Value::Float64(n as f64),
            value => value,
        },
        Ty::Simd { dtype, width: 1 } => {
            match simd_from_values(*dtype, 1, std::slice::from_ref(&value)) {
                Ok(materialized) => materialized,
                Err(_) => value,
            }
        }
        Ty::Tuple(types) => match value {
            Value::Tuple(items) => Value::Tuple(
                items
                    .into_iter()
                    .zip(types)
                    .map(|(value, ty)| coerce_checked(value, ty))
                    .collect(),
            ),
            value => value,
        },
        Ty::ComptimeList(element) => match value {
            Value::ComptimeList(items) => Value::ComptimeList(
                items
                    .into_iter()
                    .map(|value| coerce_checked(value, element))
                    .collect(),
            ),
            value => value,
        },
        Ty::Variant(types) => match value {
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
    fn rank(self) -> u8 {
        match self {
            Num::I(_) => 0,
            Num::U(_) => 1,
            Num::F(_) => 2,
        }
    }
    fn as_i64(self) -> i64 {
        match self {
            Num::I(n) => n,
            Num::U(n) => n as i64,
            Num::F(x) => x as i64,
        }
    }
    fn as_u64(self) -> u64 {
        match self {
            Num::I(n) => n as u64,
            Num::U(n) => n,
            Num::F(x) => x as u64,
        }
    }
    fn as_f64(self) -> f64 {
        match self {
            Num::I(n) => n as f64,
            Num::U(n) => n as f64,
            Num::F(x) => x,
        }
    }
}

/// View a value as a number, if it is one.
fn as_num(v: &Value) -> Option<Num> {
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
            ExactNum::Int(value) => mojito_common::literal::FloatLiteral::from_int(&value),
            ExactNum::Float(value) => value,
        }
    }

    fn compare(&self, rhs: &Self) -> Ordering {
        match (self, rhs) {
            (ExactNum::Int(left), ExactNum::Int(right)) => left.cmp(right),
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

fn compare_tuples(left: &[Value], right: &[Value]) -> Result<Ordering, RuntimeError> {
    for (left, right) in left.iter().zip(right) {
        let ordering = compare_tuple_values(left, right)?;
        if ordering != Ordering::Equal {
            return Ok(ordering);
        }
    }
    Ok(left.len().cmp(&right.len()))
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

/// Build a SIMD `Value` from `dtype`+`lanes`, canonicalizing a **width-1
/// `float64`** to the native `Value::Float64` and a **width-1 `int`** to the
/// native `Value::Int` — the runtime side of `simd_ty`'s unification of
/// `Float64`/`SIMD[DType.float64, 1]` and `Int`/`Scalar[DType.int]`.
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
    Value::Simd { dtype, lanes }
}

/// Lift any finite runtime numeric value into the same exact comparison domain
/// as a literal. This keeps erased/generic equality and hashing coherent even
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
fn floor_div(x: i64, y: i64) -> i64 {
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
fn floor_mod(x: i64, y: i64) -> i64 {
    let r = x.wrapping_rem(y);
    if r != 0 && ((r < 0) != (y < 0)) {
        r + y
    } else {
        r
    }
}

/// Round an `f64` to single precision (for `float32` lanes).
fn round_f32(x: f64) -> f64 {
    x as f32 as f64
}

fn value_to_int_lane(v: &Value, dtype: Dtype) -> Result<i128, RuntimeError> {
    if let Value::IntLiteral(value) = v {
        let (bits, signed) = integer_dtype_bits(dtype).ok_or_else(|| {
            RuntimeError::TypeError(format!("{dtype:?} is not an integer SIMD dtype"))
        })?;
        return if signed {
            value
                .wrapping_signed(bits)
                .map(i128::from)
                .ok_or_else(|| literal_materialization_error(value, &format!("{dtype:?}")))
        } else {
            value
                .wrapping_unsigned(bits)
                .map(i128::from)
                .ok_or_else(|| literal_materialization_error(value, &format!("{dtype:?}")))
        };
    }
    Ok(wrap(dtype, value_to_int(v)?))
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
    if dtype == Dtype::Float32 {
        return match v {
            Value::IntLiteral(value) => mojito_common::literal::FloatLiteral::from_int(value)
                .to_f32()
                .map(|value| value as f64)
                .ok_or_else(|| literal_materialization_error(value, "Float32")),
            Value::FloatLiteral(value) => value
                .to_f32()
                .map(|value| value as f64)
                .ok_or_else(|| literal_materialization_error(value, "Float32")),
            other => Ok(round_f32(value_to_float(other)?)),
        };
    }
    value_to_float(v)
}

/// A scalar value's boolean content, for building a `bool` SIMD lane.
fn value_to_bool_lane(v: &Value) -> Result<bool, RuntimeError> {
    match v {
        Value::Bool(b) => Ok(*b),
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
fn pow_exp(y: i64) -> Result<u32, RuntimeError> {
    u32::try_from(y).map_err(|_| {
        RuntimeError::TypeError(
            "'**' exponent must be a non-negative Int that fits in 32 bits".to_string(),
        )
    })
}

/// The `(bits, signed)` lane shape of an integer SIMD dtype, or `None` for
/// the float and bool dtypes. Shared with the native backends so their width
/// tables cannot drift from the VM's.
pub fn integer_dtype_bits(dtype: Dtype) -> Option<(u32, bool)> {
    Some(match dtype {
        Dtype::Int => (64, true),
        Dtype::Int8 => (8, true),
        Dtype::Int16 => (16, true),
        Dtype::Int32 => (32, true),
        Dtype::Int64 => (64, true),
        Dtype::UInt8 => (8, false),
        Dtype::UInt16 => (16, false),
        Dtype::UInt32 => (32, false),
        Dtype::UInt64 => (64, false),
        Dtype::Float32 | Dtype::Float64 | Dtype::Bool => return None,
    })
}

/// The `(dtype, width)` of a value if it is a SIMD.
fn simd_shape(v: &Value) -> Option<(Dtype, usize)> {
    match v {
        Value::Simd { dtype, lanes } => Some((*dtype, lanes.width())),
        _ => None,
    }
}

/// Elementwise dtype conversion (`v.cast[DType.<dt>]()`): integer targets
/// re-wrap at the new element width, float targets convert numerically
/// (`float32` rounds), and a float source converts to an integer lane by
/// truncating toward zero (saturating at the 128-bit intermediate) and then
/// wrapping — Mojito's pinned bit-accurate policy. Bool casts are rejected at
/// checking.
pub fn simd_cast(target: Dtype, value: &Value) -> Result<Value, RuntimeError> {
    // A canonicalized width-1 receiver arrives as a native scalar.
    let lanes = match value {
        Value::Simd { lanes, .. } => lanes.clone(),
        Value::Int(n) => SimdLanes::Int(vec![*n as i128]),
        Value::Float64(x) => SimdLanes::Float(vec![*x]),
        other => {
            return Err(RuntimeError::TypeError(format!(
                "cannot cast {} as a SIMD value",
                type_name(other)
            )));
        }
    };
    let cast = match (&lanes, target.is_float()) {
        (SimdLanes::Int(v), false) => SimdLanes::Int(v.iter().map(|x| wrap(target, *x)).collect()),
        (SimdLanes::Int(v), true) => {
            let round = |x: f64| {
                if target == Dtype::Float32 {
                    round_f32(x)
                } else {
                    x
                }
            };
            SimdLanes::Float(v.iter().map(|x| round(*x as f64)).collect())
        }
        (SimdLanes::Float(v), false) => {
            SimdLanes::Int(v.iter().map(|x| wrap(target, x.trunc() as i128)).collect())
        }
        (SimdLanes::Float(v), true) => {
            let round = |x: f64| {
                if target == Dtype::Float32 {
                    round_f32(x)
                } else {
                    x
                }
            };
            SimdLanes::Float(v.iter().map(|x| round(*x)).collect())
        }
        (SimdLanes::Bool(_), _) => {
            return Err(RuntimeError::TypeError(
                "bool SIMD dtype casts are not supported".to_string(),
            ));
        }
    };
    Ok(simd_value(target, cast))
}

/// Lane gather (`v.shuffle[*mask]()`): result lane `i` is the source's lane
/// `mask[i]`. The checker bounded every index by the receiver's width; the
/// bounds check here is the phase-boundary backstop.
pub fn simd_shuffle(value: &Value, mask: &[usize]) -> Result<Value, RuntimeError> {
    let Value::Simd { dtype, lanes } = value else {
        return Err(RuntimeError::TypeError(format!(
            "cannot shuffle {} as a SIMD value",
            type_name(value)
        )));
    };
    let width = lanes.width();
    if let Some(bad) = mask.iter().find(|lane| **lane >= width) {
        return Err(RuntimeError::TypeError(format!(
            "shuffle lane {bad} is out of range for width {width}"
        )));
    }
    let gathered = match lanes {
        SimdLanes::Int(v) => SimdLanes::Int(mask.iter().map(|lane| v[*lane]).collect()),
        SimdLanes::Float(v) => SimdLanes::Float(mask.iter().map(|lane| v[*lane]).collect()),
        SimdLanes::Bool(v) => SimdLanes::Bool(mask.iter().map(|lane| v[*lane]).collect()),
    };
    Ok(simd_value(*dtype, gathered))
}

/// Dispatch a method call on a SIMD receiver: `select` on bool masks and the
/// lane reductions. The checker validated shapes; an unknown name is a clean
/// has-no-method error.
pub fn simd_method(
    dtype: Dtype,
    lanes: &SimdLanes,
    method: &str,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    match method {
        "select" if args.len() == 2 => simd_select(lanes, &args[0], &args[1]),
        "reduce_add" | "reduce_mul" | "reduce_min" | "reduce_max" if args.is_empty() => {
            simd_reduce(dtype, lanes, method)
        }
        "reduce_and" | "reduce_or" if args.is_empty() => match lanes {
            SimdLanes::Bool(v) => Ok(Value::Bool(if method == "reduce_and" {
                v.iter().all(|lane| *lane)
            } else {
                v.iter().any(|lane| *lane)
            })),
            _ => Err(RuntimeError::TypeError(format!(
                "{method}() requires bool lanes"
            ))),
        },
        _ => Err(RuntimeError::Unsupported(format!(
            "vm: SIMD[DType.{}, {}] has no method '{method}'",
            dtype.name(),
            lanes.width()
        ))),
    }
}

/// Elementwise blend through a bool mask: lane `i` of the result takes
/// `when_true`'s lane where the mask is set, else `when_false`'s. A scalar or
/// literal case splats, like an infix operand.
fn simd_select(
    mask: &SimdLanes,
    when_true: &Value,
    when_false: &Value,
) -> Result<Value, RuntimeError> {
    let SimdLanes::Bool(mask) = mask else {
        return Err(RuntimeError::TypeError(
            "select() requires a bool-mask receiver".to_string(),
        ));
    };
    let width = mask.len();
    // The checker guarantees at least one case is a SIMD of the payload dtype.
    let Some(dtype) = [when_true, when_false].into_iter().find_map(|v| match v {
        Value::Simd { dtype, .. } => Some(*dtype),
        _ => None,
    }) else {
        return Err(RuntimeError::TypeError(
            "select() requires a SIMD case operand".to_string(),
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

/// Collapse lanes to the canonicalized width-1 scalar. Integer `add`/`mul`
/// wrap at the element width per step, like the elementwise operators;
/// `float32` rounds each step to single precision.
fn simd_reduce(dtype: Dtype, lanes: &SimdLanes, method: &str) -> Result<Value, RuntimeError> {
    match lanes {
        SimdLanes::Int(v) => {
            let mut acc = v[0];
            for x in &v[1..] {
                acc = match method {
                    "reduce_add" => wrap(dtype, acc.wrapping_add(*x)),
                    "reduce_mul" => wrap(dtype, acc.wrapping_mul(*x)),
                    "reduce_min" => acc.min(*x),
                    _ => acc.max(*x),
                };
            }
            Ok(simd_value(dtype, SimdLanes::Int(vec![acc])))
        }
        SimdLanes::Float(v) => {
            let round = |x: f64| {
                if dtype == Dtype::Float32 {
                    round_f32(x)
                } else {
                    x
                }
            };
            let mut acc = v[0];
            for x in &v[1..] {
                acc = match method {
                    "reduce_add" => round(acc + x),
                    "reduce_mul" => round(acc * x),
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
/// it is a SIMD, else the scalar splatted across all lanes.
fn to_int_lanes(v: &Value, dtype: Dtype, width: usize) -> Result<Vec<i128>, RuntimeError> {
    match v {
        Value::Simd {
            lanes: SimdLanes::Int(l),
            ..
        } => Ok(l.clone()),
        scalar => Ok(vec![value_to_int_lane(scalar, dtype)?; width]),
    }
}

fn to_float_lanes(v: &Value, dtype: Dtype, width: usize) -> Result<Vec<f64>, RuntimeError> {
    match v {
        Value::Simd {
            lanes: SimdLanes::Float(l),
            ..
        } => Ok(l.clone()),
        scalar => Ok(vec![value_to_float_lane(scalar, dtype)?; width]),
    }
}

fn to_bool_lanes(v: &Value, width: usize) -> Result<Vec<bool>, RuntimeError> {
    match v {
        Value::Simd {
            lanes: SimdLanes::Bool(l),
            ..
        } => Ok(l.clone()),
        scalar => Ok(vec![value_to_bool_lane(scalar)?; width]),
    }
}

/// Compare two tuple elements for lexicographic tuple ordering. Numeric values
/// use the same promotion ranks as arithmetic; strings and nested tuples compare
/// structurally.
fn compare_tuple_values(left: &Value, right: &Value) -> Result<Ordering, RuntimeError> {
    if (as_exact_num(left).is_some() || as_exact_num(right).is_some())
        && (as_exact_num(left).is_some() || as_num(left).is_some())
        && (as_exact_num(right).is_some() || as_num(right).is_some())
    {
        let compare = |op| match apply_infix(op, left.clone(), right.clone())? {
            Value::Bool(value) => Ok(value),
            _ => Err(RuntimeError::TypeError(
                "numeric tuple comparison did not produce Bool".into(),
            )),
        };
        if compare(InfixOp::Eq)? {
            return Ok(Ordering::Equal);
        }
        if compare(InfixOp::Lt)? {
            return Ok(Ordering::Less);
        }
        if compare(InfixOp::Gt)? {
            return Ok(Ordering::Greater);
        }
        return Err(RuntimeError::TypeError(
            "cannot order NaN tuple elements".into(),
        ));
    }
    if let (Some(left), Some(right)) = (as_num(left), as_num(right)) {
        return match left.rank().max(right.rank()) {
            2 => left
                .as_f64()
                .partial_cmp(&right.as_f64())
                .ok_or_else(|| RuntimeError::TypeError("cannot order NaN tuple elements".into())),
            1 => Ok(left.as_u64().cmp(&right.as_u64())),
            _ => Ok(left.as_i64().cmp(&right.as_i64())),
        };
    }
    match (left, right) {
        (Value::Str(left), Value::Str(right)) => Ok(left.cmp(right)),
        (Value::Tuple(left), Value::Tuple(right)) => compare_tuples(left, right),
        _ => Err(RuntimeError::TypeError(format!(
            "cannot order {} and {} tuple elements",
            type_name(left),
            type_name(right)
        ))),
    }
}

fn exact_numeric_op(op: InfixOp, left: ExactNum, right: ExactNum) -> Result<Value, RuntimeError> {
    use InfixOp::*;
    if let (ExactNum::Int(left), ExactNum::Int(right)) = (&left, &right) {
        let value =
            match op {
                Add => Value::IntLiteral(left.add(right)),
                Sub => Value::IntLiteral(left.sub(right)),
                Mul => Value::IntLiteral(left.mul(right)),
                Div => Value::FloatLiteral(
                    mojito_common::literal::FloatLiteral::from_int(left)
                        .div(&mojito_common::literal::FloatLiteral::from_int(right))
                        .ok_or_else(|| {
                            RuntimeError::TypeError("integer division by zero".to_string())
                        })?,
                ),
                FloorDiv => Value::IntLiteral(left.floor_div(right).ok_or_else(|| {
                    RuntimeError::TypeError("integer division by zero".to_string())
                })?),
                Mod => Value::IntLiteral(left.floor_mod(right).ok_or_else(|| {
                    RuntimeError::TypeError("integer modulo by zero".to_string())
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
                MatMul | And | Or | In | NotIn => {
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
        Shl | Shr | BitAnd | BitOr | BitXor | MatMul | And | Or | In | NotIn => {
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
    // True division always computes in f64 and yields Float64.
    if op == InfixOp::Div {
        return Ok(Value::Float64(a.as_f64() / b.as_f64()));
    }
    match a.rank().max(b.rank()) {
        2 => float_op(op, a.as_f64(), b.as_f64()),
        1 => uint_op(op, a.as_u64(), b.as_u64()),
        _ => int_op(op, a.as_i64(), b.as_i64()),
    }
}

fn int_op(op: InfixOp, x: i64, y: i64) -> Result<Value, RuntimeError> {
    use InfixOp::*;
    Ok(match op {
        // Int overflow is defined two's-complement wrapping (the native ABI
        // contract, docs/native-abi.md); `//`, `%`, and `**` keep their
        // checked zero-divisor/exponent traps.
        Add => Value::Int(x.wrapping_add(y)),
        Sub => Value::Int(x.wrapping_sub(y)),
        Mul => Value::Int(x.wrapping_mul(y)),
        FloorDiv => Value::Int(floor_div(x, nonzero(y)?)),
        Mod => Value::Int(floor_mod(x, nonzero(y)?)),
        Pow => Value::Int(x.wrapping_pow(pow_exp(y)?)),
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
        Div | MatMul | And | Or | In | NotIn => {
            return Err(RuntimeError::TypeError(format!(
                "operator '{op:?}' is invalid for integer dispatch"
            )));
        }
    })
}

fn uint_op(op: InfixOp, x: u64, y: u64) -> Result<Value, RuntimeError> {
    use InfixOp::*;
    Ok(match op {
        // UInt overflow wraps mod 2^64 (the native ABI contract).
        Add => Value::UInt(x.wrapping_add(y)),
        Sub => Value::UInt(x.wrapping_sub(y)),
        Mul => Value::UInt(x.wrapping_mul(y)),
        // Unsigned: floor division/modulo are plain `/` and `%`.
        FloorDiv => Value::UInt(x / nonzero_u(y)?),
        Mod => Value::UInt(x % nonzero_u(y)?),
        Pow => Value::UInt(x.wrapping_pow(pow_exp(y as i64)?)),
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
        Div | MatMul | And | Or | In | NotIn => {
            return Err(RuntimeError::TypeError(format!(
                "operator '{op:?}' is invalid for unsigned dispatch"
            )));
        }
    })
}

fn float_op(op: InfixOp, x: f64, y: f64) -> Result<Value, RuntimeError> {
    use InfixOp::*;
    Ok(match op {
        Add => Value::Float64(x + y),
        Sub => Value::Float64(x - y),
        Mul => Value::Float64(x * y),
        FloorDiv => Value::Float64((x / y).floor()),
        Mod => Value::Float64(x - y * (x / y).floor()),
        Pow => Value::Float64(x.powf(y)),
        Lt => Value::Bool(x < y),
        Gt => Value::Bool(x > y),
        Le => Value::Bool(x <= y),
        Ge => Value::Bool(x >= y),
        Eq => Value::Bool(x == y),
        Ne => Value::Bool(x != y),
        Div | MatMul | Shl | Shr | BitAnd | BitOr | BitXor | And | Or | In | NotIn => {
            return Err(RuntimeError::TypeError(format!(
                "operator '{op:?}' is invalid for float dispatch"
            )));
        }
    })
}

/// Round a float result to its lane precision: `float32` truncates to single
/// precision, `float64` keeps full `f64`. (Called only for float dtypes.)
fn round_lane(dtype: Dtype, x: f64) -> f64 {
    match dtype {
        Dtype::Float32 => round_f32(x),
        _ => x,
    }
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

fn materialize_literal_int_lane(
    value: &mojito_common::literal::IntLiteral,
    dtype: Dtype,
) -> Option<i128> {
    let (bits, signed) = integer_dtype_bits(dtype)?;
    if signed {
        value.wrapping_signed(bits).map(i128::from)
    } else {
        value.wrapping_unsigned(bits).map(i128::from)
    }
}

fn int_arith(op: InfixOp, dtype: Dtype, a: i128, b: i128) -> i128 {
    match op {
        InfixOp::Add => a + b,
        InfixOp::Sub => a - b,
        InfixOp::Mul => a * b,
        InfixOp::BitAnd => a & b,
        InfixOp::BitOr => a | b,
        InfixOp::BitXor => a ^ b,
        InfixOp::Shl => {
            let bits = integer_dtype_bits(dtype).map_or(128, |(bits, _)| bits);
            a << ((b as u32) & (bits - 1))
        }
        InfixOp::Shr => {
            let (bits, signed) = integer_dtype_bits(dtype).unwrap_or((128, true));
            let amount = (b as u32) & (bits - 1);
            if signed {
                a >> amount
            } else {
                ((a as u128) >> amount) as i128
            }
        }
        _ => 0, // checker rejects other arithmetic on SIMD ints
    }
}

fn int_cmp(op: InfixOp, a: i128, b: i128) -> bool {
    match op {
        InfixOp::Eq => a == b,
        InfixOp::Ne => a != b,
        InfixOp::Lt => a < b,
        InfixOp::Gt => a > b,
        InfixOp::Le => a <= b,
        InfixOp::Ge => a >= b,
        _ => false,
    }
}

fn float_arith(op: InfixOp, a: f64, b: f64) -> f64 {
    match op {
        InfixOp::Add => a + b,
        InfixOp::Sub => a - b,
        InfixOp::Mul => a * b,
        InfixOp::Div => a / b,
        _ => 0.0,
    }
}

fn float_cmp(op: InfixOp, a: f64, b: f64) -> bool {
    match op {
        InfixOp::Eq => a == b,
        InfixOp::Ne => a != b,
        InfixOp::Lt => a < b,
        InfixOp::Gt => a > b,
        InfixOp::Le => a <= b,
        InfixOp::Ge => a >= b,
        _ => false,
    }
}

// --- Collections / indexing ---

/// Convert a signed index into a bounds-checked `usize`, or a runtime error.
pub fn bounds_check(idx: i64, len: usize, what: &str) -> Result<usize, RuntimeError> {
    if idx < 0 || idx as usize >= len {
        return Err(RuntimeError::TypeError(format!(
            "{} index {} out of range 0..{}",
            what, idx, len
        )));
    }
    Ok(idx as usize)
}

// --- SIMD ---

/// Wrap an integer to a dtype's bit width (bit-accurate overflow), storing the
/// post-wrap mathematical value (non-negative for unsigned dtypes).
fn wrap(dtype: Dtype, v: i128) -> i128 {
    match dtype {
        Dtype::Int => v,
        Dtype::Int8 => v as i8 as i128,
        Dtype::Int16 => v as i16 as i128,
        Dtype::Int32 => v as i32 as i128,
        Dtype::Int64 => v as i64 as i128,
        Dtype::UInt8 => v as u8 as i128,
        Dtype::UInt16 => v as u16 as i128,
        Dtype::UInt32 => v as u32 as i128,
        Dtype::UInt64 => v as u64 as i128,
        Dtype::Float32 | Dtype::Float64 | Dtype::Bool => v,
    }
}

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
            RuntimeError::UndefinedVariable(name) => write!(f, "Undefined variable '{}'", name),
            RuntimeError::TypeError(msg) => write!(f, "Type error: {}", msg),
            RuntimeError::NotCallable(name) => write!(f, "'{}' is not callable", name),
            RuntimeError::ArityMismatch {
                name,
                expected,
                got,
            } => {
                write!(
                    f,
                    "'{}' expects {} argument(s), got {}",
                    name, expected, got
                )
            }
            RuntimeError::ClosureEscape => {
                write!(
                    f,
                    "closures cannot escape their defining scope (downward funargs only)"
                )
            }
            RuntimeError::Raised(value) => write!(f, "unhandled error: {}", value),
            RuntimeError::Unsupported(what) => write!(f, "unsupported feature: {}", what),
            RuntimeError::Abort(message) => write!(f, "abort: {}", message),
        }
    }
}

impl std::error::Error for RuntimeError {}

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
        assert_eq!(
            int_op(InfixOp::Pow, 3, 41).unwrap(),
            int(3i64.wrapping_pow(41))
        );
        // Zero divisors still trap; only overflow got defined.
        assert!(int_op(InfixOp::FloorDiv, 1, 0).is_err());
        assert!(int_op(InfixOp::Pow, 2, -1).is_err());

        let uint = |value| Value::UInt(value);
        assert_eq!(uint_op(InfixOp::Sub, 0, 1).unwrap(), uint(u64::MAX));
        assert_eq!(uint_op(InfixOp::Add, u64::MAX, 1).unwrap(), uint(0));
        assert_eq!(
            uint_op(InfixOp::Mul, u64::MAX, 2).unwrap(),
            uint(u64::MAX - 1)
        );
        assert!(uint_op(InfixOp::Mod, 1, 0).is_err());
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
    fn hash_bits_fold_negative_zero_and_zero_extend_lanes() {
        assert_eq!(
            hash_bits(&Value::Float64(-0.0)).unwrap(),
            hash_bits(&Value::Float64(0.0)).unwrap()
        );
        assert_eq!(hash_bits(&Value::Int(-1)).unwrap(), u64::MAX);
        assert_eq!(hash_bits(&Value::Bool(true)).unwrap(), 1);
        assert_eq!(
            hash_bits(&Value::Simd {
                dtype: Dtype::Int8,
                lanes: SimdLanes::Int(vec![-1]),
            })
            .unwrap(),
            0xFF
        );
    }
}

//! The value crossing between compile-time values and the VM.
//!
//! A compile-time evaluation hands the VM its arguments as runtime values and
//! takes the result back as a compile-time value. Both directions admit the
//! same closed set: scalars and literals, `Bool`, `Str`, tuples,
//! compile-time lists, fieldwise structs, SIMD values, and `DType`. A nominal
//! `String` lives in the VM's heap and is frozen by the VM that owns it
//! (`VmBackend::freeze`). The fuel every compile-time evaluation shares is
//! declared here too.

use crate::runtime::{RuntimeError, SimdLanes, Value};
use mojito_types::ct::{CtLane, CtValue};

/// The quota of compile-time execution steps per compilation: elaboration
/// entries, instructions executed, frames entered. A hard bound, so
/// compile-time execution cannot hang the compiler.
pub const CTFE_FUEL: usize = 100_000;

/// A compile-time value as the runtime value the VM takes.
pub fn ct_to_vm(value: &CtValue) -> Result<Value, RuntimeError> {
    let lanes_of = |lanes: &[CtLane]| -> Result<SimdLanes, RuntimeError> {
        let mixed = || RuntimeError::Unsupported("mixed SIMD lane kinds".to_string());
        Ok(match lanes.first() {
            Some(CtLane::Float(_)) => SimdLanes::Float(
                lanes
                    .iter()
                    .map(|lane| match lane {
                        CtLane::Float(bits) => Some(f64::from_bits(*bits)),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(mixed)?,
            ),
            Some(CtLane::Bool(_)) => SimdLanes::Bool(
                lanes
                    .iter()
                    .map(|lane| match lane {
                        CtLane::Bool(value) => Some(*value),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(mixed)?,
            ),
            _ => SimdLanes::Int(
                lanes
                    .iter()
                    .map(|lane| match lane {
                        CtLane::Int(value) => Some(*value),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(mixed)?,
            ),
        })
    };
    match value {
        CtValue::Pointer { .. } => Err(RuntimeError::Unsupported(
            "frozen pointer memory requires a VM heap to thaw".to_string(),
        )),
        CtValue::Int(n) => Ok(Value::Int(*n)),
        CtValue::UInt(n) => Ok(Value::UInt(*n)),
        CtValue::Float(bits) => Ok(Value::Float64(f64::from_bits(*bits))),
        CtValue::IntLiteral(value) => Ok(Value::IntLiteral(value.clone())),
        CtValue::FloatLiteral(value) => Ok(Value::FloatLiteral(value.clone())),
        CtValue::Bool(b) => Ok(Value::Bool(*b)),
        CtValue::Str(s) => Ok(Value::Str(s.clone())),
        CtValue::Tuple(items) => Ok(Value::Tuple(
            items.iter().map(ct_to_vm).collect::<Result<Vec<_>, _>>()?,
        )),
        CtValue::List(items) => Ok(Value::ComptimeList(
            items.iter().map(ct_to_vm).collect::<Result<Vec<_>, _>>()?,
        )),
        CtValue::Struct { name, fields } => Ok(Value::Struct {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|(field, value)| Ok::<_, RuntimeError>((field.clone(), ct_to_vm(value)?)))
                .collect::<Result<Vec<_>, _>>()?,
        }),
        CtValue::Simd { dtype, lanes } => Ok(Value::Simd {
            dtype: *dtype,
            lanes: lanes_of(lanes)?,
        }),
        CtValue::Dtype(dtype) => Ok(Value::Dtype(*dtype)),
        CtValue::Type(_)
        | CtValue::Reflected(_)
        | CtValue::Expr(_)
        | CtValue::Deferred(_)
        | CtValue::Marker(_) => Err(RuntimeError::Unsupported(
            "type-valued or symbolic values cannot cross into VM CTFE".to_string(),
        )),
        // A collection crosses into VM CTFE only as its materialized display
        // in a synthesized entry, never as a runtime value.
        CtValue::Dict { .. } | CtValue::Set { .. } => Err(RuntimeError::Unsupported(
            "a compile-time collection crosses into VM CTFE only through a synthesized entry"
                .to_string(),
        )),
    }
}

/// A runtime value the VM produced as a compile-time value.
pub fn vm_to_ct(value: Value) -> Result<CtValue, RuntimeError> {
    match value {
        Value::Int(n) => Ok(CtValue::Int(n)),
        Value::UInt(n) => Ok(CtValue::UInt(n)),
        Value::Float64(value) => Ok(CtValue::Float(value.to_bits())),
        Value::IntLiteral(value) => Ok(CtValue::IntLiteral(value)),
        Value::FloatLiteral(value) => Ok(CtValue::FloatLiteral(value)),
        Value::Bool(b) => Ok(CtValue::Bool(b)),
        Value::Str(s) => Ok(CtValue::Str(s)),
        Value::Tuple(items) => Ok(CtValue::Tuple(
            items
                .into_iter()
                .map(vm_to_ct)
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Value::ComptimeList(items) => Ok(CtValue::List(
            items
                .into_iter()
                .map(vm_to_ct)
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Value::Simd { dtype, lanes } => Ok(CtValue::Simd {
            dtype,
            lanes: match lanes {
                SimdLanes::Int(values) => values.into_iter().map(CtLane::Int).collect(),
                SimdLanes::Float(values) => values
                    .into_iter()
                    .map(|x| CtLane::Float(x.to_bits()))
                    .collect(),
                SimdLanes::Bool(values) => values.into_iter().map(CtLane::Bool).collect(),
            },
        }),
        Value::Dtype(dtype) => Ok(CtValue::Dtype(dtype)),
        Value::None => Err(RuntimeError::Unsupported(
            "VM CTFE function returned None; a compile-time value is required".to_string(),
        )),
        other => Err(RuntimeError::Unsupported(format!(
            "VM CTFE returned unsupported runtime value {other}"
        ))),
    }
}

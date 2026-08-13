//! Navigation and mutation of projected VM storage and reference handles.

use super::*;

/// Navigate a [`MirPlace`] to a mutable slot: the root variable followed by field
/// and index projections. Used for method write-back and `MovePlace` (a field
/// chain or statically selected compiler-private Tuple element). A SIMD lane
/// isn't a `Value` slot; use `store_place`/`load_place` for a place that may end
/// in a lane.
pub(super) fn nav_mut<'a>(
    vars: &'a mut [Value],
    regs: &[Value],
    place: &MirPlace,
) -> Result<&'a mut Value, RuntimeError> {
    let mut slot = &mut vars[place.root as usize];
    for proj in &place.proj {
        slot = nav_step(slot, proj, regs)?;
    }
    Ok(slot)
}

/// Write `value` into a place, handling a **SIMD lane** target (`v[i] = e`,
/// `obj.vec[i] = e`) — a lane isn't a `Value` slot, so it is set via
/// `set_simd_lane` (dtype wrap/round) after navigating the container.
pub(super) fn store_place(
    vars: &mut [Value],
    regs: &[Value],
    place: &MirPlace,
    value: Value,
) -> Result<(), RuntimeError> {
    match place.proj.split_last() {
        None => {
            vars[place.root as usize] = value;
            Ok(())
        }
        Some((last, prefix)) => {
            let mut slot = &mut vars[place.root as usize];
            for proj in prefix {
                slot = nav_step(slot, proj, regs)?;
            }
            if let Proj::Index(ireg) = last
                && let Value::Simd { dtype, lanes } = slot
            {
                let idx = value_as_index(&regs[ireg.0 as usize])?;
                return crate::runtime::set_simd_lane(*dtype, lanes, idx, value);
            }
            // A final payload write initializes-or-overwrites inline uninit
            // storage without running any destructor: `unsafe_write` leaks a
            // previously initialized payload by design.
            if let (Proj::UninitPayload, Value::UninitStorage(payload)) = (last, &mut *slot) {
                *payload = Some(Box::new(value));
                return Ok(());
            }
            *nav_step(slot, last, regs)? = value;
            Ok(())
        }
    }
}

/// Read (clone) the value at a place, handling a **SIMD lane** read (`v[i]`,
/// `obj.vec[i]`) via `read_simd_lane`.
pub(super) fn load_place(
    vars: &mut [Value],
    regs: &[Value],
    place: &MirPlace,
) -> Result<Value, RuntimeError> {
    if let Some((Proj::Index(ireg), prefix)) = place.proj.split_last() {
        let mut slot = &mut vars[place.root as usize];
        for proj in prefix {
            slot = nav_step(slot, proj, regs)?;
        }
        if let Value::Simd { dtype, lanes } = slot {
            let idx = value_as_index(&regs[ireg.0 as usize])?;
            return read_simd_lane(*dtype, lanes, idx);
        }
        // Not a SIMD parent — fall through to the final index step below.
        return Ok(nav_step(slot, &Proj::Index(*ireg), regs)?.clone());
    }
    Ok(nav_mut(vars, regs, place)?.clone())
}

/// Navigate one projection step from a container slot to an inner mutable slot.
/// A SIMD lane is *not* a `Value` slot, so it is not reachable here — callers that
/// may target a lane (`store_place`/`load_place`) special-case a final `Index`
/// into a `Value::Simd` before reaching this.
fn nav_step<'a>(
    slot: &'a mut Value,
    proj: &Proj,
    regs: &[Value],
) -> Result<&'a mut Value, RuntimeError> {
    match proj {
        Proj::Field(name) => match slot {
            // Search declared fields first, then value parameters (a `Self.n`
            // read) — mirroring `get_field`, so a place read through this matches
            // the register-based `GetField`.
            Value::Struct {
                fields,
                value_params,
                ..
            } => {
                if let Some(pos) = fields.iter().position(|(f, _)| f == name) {
                    Ok(&mut fields[pos].1)
                } else if let Some(pos) = value_params.iter().position(|(f, _)| f == name) {
                    Ok(&mut value_params[pos].1)
                } else {
                    Err(RuntimeError::TypeError(format!("no field '{name}'")))
                }
            }
            other => Err(RuntimeError::TypeError(format!(
                "field access on non-struct {}",
                crate::runtime::type_name(other)
            ))),
        },
        Proj::Index(reg) => {
            let idx = value_as_index(&regs[reg.0 as usize])?;
            match slot {
                Value::Tuple(items) => {
                    let i = crate::runtime::bounds_check(idx, items.len(), "tuple index")?;
                    Ok(&mut items[i])
                }
                other => Err(RuntimeError::TypeError(format!(
                    "cannot index {}",
                    crate::runtime::type_name(other)
                ))),
            }
        }
        Proj::ConstIndex(index) => match slot {
            Value::Tuple(items) => items
                .get_mut(*index)
                .ok_or_else(|| RuntimeError::TypeError("tuple index out of bounds".to_string())),
            other => Err(RuntimeError::TypeError(format!(
                "cannot apply a constant Tuple projection to {}",
                crate::runtime::type_name(other)
            ))),
        },
        Proj::Variant(expected) => match slot {
            Value::Variant {
                alternatives,
                index,
                value,
            } => {
                if index != expected {
                    return Err(RuntimeError::TypeError(format!(
                        "Variant holds '{}', not '{}'",
                        alternatives
                            .get(*index)
                            .map(ToString::to_string)
                            .unwrap_or_else(|| "<invalid>".to_string()),
                        alternatives
                            .get(*expected)
                            .map(ToString::to_string)
                            .unwrap_or_else(|| "<invalid>".to_string())
                    )));
                }
                Ok(value.as_mut())
            }
            _ => Err(RuntimeError::TypeError(
                "Variant projection on a non-Variant value".to_string(),
            )),
        },
        Proj::UninitPayload => match slot {
            Value::UninitStorage(Some(payload)) => Ok(payload.as_mut()),
            Value::UninitStorage(None) => Err(RuntimeError::TypeError(
                "vm: read of uninitialized UnsafeMaybeUninit storage".to_string(),
            )),
            other => Err(RuntimeError::TypeError(format!(
                "payload projection on non-uninit-storage {}",
                crate::runtime::type_name(other)
            ))),
        },
    }
}

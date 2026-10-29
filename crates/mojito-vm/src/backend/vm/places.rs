//! Navigation and mutation of projected VM storage and reference handles.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

/// Navigate a [`MirPlace`] to a mutable slot: the root variable followed by field
/// and index projections. Used for method write-back and `MovePlace` (a field
/// chain or statically selected compiler-private Tuple element).
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

/// Write `value` into a place.
pub(super) fn store_place(
    vars: &mut [Value],
    regs: &[Value],
    place: &MirPlace,
    value: Value,
) -> Result<(), RuntimeError> {
    let place = &*place.storage();
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

/// Read (clone) the value at a place.
pub(super) fn load_place(
    vars: &mut [Value],
    regs: &[Value],
    place: &MirPlace,
) -> Result<Value, RuntimeError> {
    Ok(nav_mut(vars, regs, &place.storage())?.clone())
}

/// Navigate one projection step from a container slot to an inner mutable slot.
fn nav_step<'a>(
    slot: &'a mut Value,
    proj: &Proj,
    regs: &[Value],
) -> Result<&'a mut Value, RuntimeError> {
    match proj {
        Proj::Rebind => Ok(slot),
        Proj::Field(name) => match slot {
            Value::Struct { fields, .. } => fields
                .iter()
                .position(|(f, _)| f == name)
                .map(|pos| &mut fields[pos].1)
                .ok_or_else(|| RuntimeError::TypeError(format!("no field '{name}'"))),
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
                let expected = &known_variant_index(expected)?;
                if index != expected {
                    return Err(RuntimeError::TypeError(format!(
                        "Variant holds '{}', not '{}'",
                        alternatives
                            .get(*index)
                            .map_or_else(|| "<invalid>".to_string(), ToString::to_string),
                        alternatives
                            .get(*expected)
                            .map_or_else(|| "<invalid>".to_string(), ToString::to_string)
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
                "vm: read of uninitialized MaybeUninit storage".to_string(),
            )),
            other => Err(RuntimeError::TypeError(format!(
                "payload projection on non-uninit-storage {}",
                crate::runtime::type_name(other)
            ))),
        },
    }
}

//! The per-instance closing of a template's lane gathers.
//!
//! A `shuffle`, `slice`, or `join` whose receiver width or own compile-time
//! arguments name a template's binders keeps its method's form of the mask
//! in MIR. Substitution closes the form's expressions; once the instance's
//! register types name the receiver's width, the elaborator checks the mask
//! against the method's constraints and folds it to its lane indices, as the
//! pin's `comptime assert`s run at instantiation. A violated constraint
//! fails the instance.
//!
//! A `pop.simd.shuffle` op's mask is known in the template, but a width it
//! is checked against may be a binder: the instance runs that part of the
//! op's verifier, as the pin's pass manager does.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::LaneMask;
use mojito_types::types::TyKind;

impl Specializer<'_> {
    /// Fold every lane mask the instance `function` of `template` reaches to
    /// its indices; one the bindings leave open stays for the verifier's
    /// concrete mode to name.
    pub(super) fn close_lane_masks(
        &self,
        template: &str,
        function: &mut MirFunction,
    ) -> Result<(), MonoError> {
        close_blocks(&mut function.blocks, &function.reg_types).map_err(|message| MonoError {
            kind: MonoErrorKind::Instantiation,
            ..self.error(Some(template), message)
        })
    }
}

/// Close the masks of one block list and the regions below it; the first
/// constraint a gather breaks is returned.
fn close_blocks(blocks: &mut [MirBlock], reg_types: &HashMap<u32, Ty>) -> Result<(), String> {
    for block in blocks {
        for instruction in &mut block.instrs {
            match instruction {
                MirInstr::SimdShuffle { value, mask, .. } if mask.is_symbolic() => {
                    let Some(TyKind::Simd { width, .. }) = reg_types.get(&value.0).map(Ty::kind)
                    else {
                        continue;
                    };
                    match width.known().and_then(|width| mask.resolve(width)) {
                        Some(Ok(lanes)) => *mask = LaneMask::Known(lanes),
                        Some(Err(constraint)) => {
                            return Err(format!("constraint failed: {constraint}"));
                        }
                        _ => {}
                    }
                }
                MirInstr::SimdShuffle {
                    dest,
                    value,
                    other: Some(_),
                    mask: LaneMask::Known(lanes),
                } => verify_shuffle_op(reg_types, *dest, *value, lanes)?,
                instruction => {
                    for region in mojito_mir::mir::instruction_regions_mut(instruction) {
                        close_blocks(region, reg_types)?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Upstream's `pop.simd.shuffle` verifier over an instance's widths, which
/// the template's check could not read: the result has a lane per mask
/// element, and every element indexes the two operands' lanes.
fn verify_shuffle_op(
    reg_types: &HashMap<u32, Ty>,
    dest: Reg,
    value: Reg,
    lanes: &[usize],
) -> Result<(), String> {
    let width = |register: Reg| {
        reg_types
            .get(&register.0)
            .and_then(|ty| mojito_types::types::held_simd_slots(ty.kind()))
            .and_then(|(_, width)| width.known())
            .and_then(|width| usize::try_from(width).ok())
    };
    if width(dest).is_some_and(|result| result != lanes.len()) {
        return Err(format!(
            "'pop.simd.shuffle' op expected result to be a vector of {} elements",
            lanes.len()
        ));
    }
    match width(value).and_then(|width| lanes.iter().find(|lane| **lane >= 2 * width)) {
        Some(lane) => Err(format!(
            "'pop.simd.shuffle' op mask element {lane} is out of bounds"
        )),
        None => Ok(()),
    }
}

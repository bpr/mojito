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
//! op's verifier, as the pin's pass manager does. So does a `pop.array.get`
//! or `pop.array.replace` whose index or array size a binder names, and
//! every `pop.array` op over an element a `RegisterPassable`-bounded binder
//! names, which Mojito's subset admits only trivially register-passable.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::LaneMask;
use mojito_types::types::TrivialLifecycle;
use mojito_types::types::TyKind;

impl Specializer<'_> {
    /// Fold every lane mask the instance `function` of `template` reaches to
    /// its indices; one the bindings leave open stays for the verifier's
    /// concrete mode to name.
    pub(super) fn close_lane_masks(
        &mut self,
        template: &str,
        function: &mut MirFunction,
    ) -> Result<(), MonoError> {
        if self.array_element_failure.is_none()
            && let Err(message) = self.verify_array_elements(&function.blocks, &function.reg_types)
        {
            self.array_element_failure = Some(self.instantiation_failure(template, message));
        }
        close_blocks(&mut function.blocks, &function.reg_types)
            .map_err(|message| self.instantiation_failure(template, message))
    }

    fn instantiation_failure(&self, template: &str, message: String) -> MonoError {
        MonoError {
            kind: MonoErrorKind::Instantiation,
            ..self.error(Some(template), message)
        }
    }

    /// The `pop.array` element rule over an instance's closed element
    /// types: the checker admitted a binder whose bound implies
    /// `RegisterPassable`, and the instance's element must be trivially
    /// copyable, movable, and destructible, as the checker requires of a
    /// closed one, since Mojito's ops copy and its array drops run their
    /// elements' code.
    fn verify_array_elements(
        &self,
        blocks: &[MirBlock],
        reg_types: &HashMap<u32, Ty>,
    ) -> Result<(), String> {
        for block in blocks {
            for instruction in &block.instrs {
                let site = match instruction {
                    MirInstr::ArrayRepeat { dest, .. } => Some(("repeat", *dest)),
                    MirInstr::ArrayGet { array, .. } => Some(("get", *array)),
                    MirInstr::ArrayReplace { array, .. } => Some(("replace", *array)),
                    MirInstr::ArrayGep { array, .. } => Some(("gep", *array)),
                    _ => None,
                };
                let element = site.and_then(|(op, register)| {
                    let mut ty = reg_types.get(&register.0)?;
                    if let TyKind::KgenPointer(pointee) = ty.kind() {
                        ty = pointee;
                    }
                    match ty.kind() {
                        TyKind::PopArray { element, .. } => Some((op, element)),
                        _ => None,
                    }
                });
                if let Some((op, element)) = element {
                    let trivial = [
                        TrivialLifecycle::Copyable,
                        TrivialLifecycle::Movable,
                        TrivialLifecycle::Deinitable,
                    ]
                    .into_iter()
                    .all(|kind| self.trivially(kind, element, &mut HashSet::new()) != Some(false));
                    if !trivial {
                        return Err(format!(
                            "'pop.array.{op}' over '{element}' is outside the subset: Mojito's \
                             '!pop.array' holds trivially register-passable elements"
                        ));
                    }
                }
                for region in mojito_mir::mir::instruction_regions(instruction) {
                    self.verify_array_elements(region, reg_types)?;
                }
            }
        }
        Ok(())
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
                MirInstr::ArrayGet { array, index, .. } => {
                    verify_array_index("get", reg_types, *array, index)?;
                }
                MirInstr::ArrayReplace { array, index, .. } => {
                    verify_array_index("replace", reg_types, *array, index)?;
                }
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

/// Upstream's `pop.array.get` and `pop.array.replace` bound check over an
/// instance's index and array size, which the template's check could not
/// read.
fn verify_array_index(
    op: &str,
    reg_types: &HashMap<u32, Ty>,
    array: Reg,
    index: &mojito_mir::mir::Const,
) -> Result<(), String> {
    let Some(TyKind::PopArray { size, .. }) = reg_types.get(&array.0).map(Ty::kind) else {
        return Ok(());
    };
    match (index.index_attribute(), size.known()) {
        (Some(index), Some(size)) if u64::try_from(index).map_or(true, |index| index >= size) => {
            Err(format!(
                "'pop.array.{op}' op array index out of bounds: {index}"
            ))
        }
        _ => Ok(()),
    }
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

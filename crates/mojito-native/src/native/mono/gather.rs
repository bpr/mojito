//! The per-instance closing of a template's lane gathers.
//!
//! A `shuffle`, `slice`, or `join` whose receiver width or own compile-time
//! arguments name a template's binders keeps its method's form of the mask
//! in MIR. Substitution closes the form's expressions; once the instance's
//! register types name the receiver's width, the elaborator checks the mask
//! against the method's constraints and folds it to its lane indices, as the
//! pin's `comptime assert`s run at instantiation. A violated constraint
//! fails the instance.

use super::rebind::reachable;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::LaneMask;

impl Specializer<'_> {
    /// Fold every lane mask the instance `function` of `template` reaches to
    /// its indices; one the bindings leave open stays for the verifier's
    /// concrete mode to name.
    pub(super) fn close_lane_masks(
        &self,
        template: &str,
        function: &mut MirFunction,
    ) -> Result<(), MonoError> {
        close_blocks(&mut function.blocks, &function.reg_types).map_err(|constraint| MonoError {
            kind: MonoErrorKind::Instantiation,
            ..self.error(Some(template), format!("constraint failed: {constraint}"))
        })
    }
}

/// Close the masks of one block list and the regions below it; the first
/// constraint a reached gather breaks is returned. An unreached gather (an
/// untaken `comptime if` arm's) is never judged.
fn close_blocks(blocks: &mut [MirBlock], reg_types: &HashMap<u32, Ty>) -> Result<(), String> {
    let reached = reachable(blocks);
    for (block, reached) in blocks.iter_mut().zip(reached) {
        for instruction in &mut block.instrs {
            match instruction {
                MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } => {
                    let regions = std::iter::once(body)
                        .chain(handler.iter_mut().map(|(_, blocks)| blocks))
                        .chain(orelse.iter_mut())
                        .chain(finalbody.iter_mut());
                    for region in regions {
                        close_blocks(region, reg_types)?;
                    }
                }
                MirInstr::SimdShuffle { value, mask, .. } if mask.is_symbolic() => {
                    let Some(Ty::Simd { width, .. }) = reg_types.get(&value.0) else {
                        continue;
                    };
                    match width.known().and_then(|width| mask.resolve(width)) {
                        Some(Ok(lanes)) => *mask = LaneMask::Known(lanes),
                        Some(Err(constraint)) if reached => return Err(constraint),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

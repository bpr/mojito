//! The one place a folded instruction becomes a `const`, for `sccp` and
//! `canonicalize` alike, as MLIR's `OperationFolder` (`FoldUtils`) is for
//! `SCCP` and the greedy pattern driver.

use mojito_mir::mir::verify::instruction_result_regs;
use mojito_mir::mir::{Const, MirBlock, MirDeclarations, MirInstr, Reg, instruction_regions_mut};
use mojito_types::types::Ty;
use std::collections::HashMap;

/// Replace each instruction of `blocks` and their regions that folds
/// ([`MirInstr::folded_constant`]) over `constant`'s operands by a `const`
/// defining the same register, as MLIR materializes the constant and erases
/// the op; returns how many were replaced. A folded producer's in-place
/// binding is no longer in place, as a `const` is no call-family producer.
/// Terminators are left alone.
pub(super) fn replace_folded(
    blocks: &mut [MirBlock],
    constant: &impl Fn(Reg) -> Option<Const>,
    reg_types: &HashMap<u32, Ty>,
    declarations: &MirDeclarations,
) -> usize {
    let mut folded = 0;
    for block in blocks {
        let mut folded_here = Vec::new();
        for instruction in &mut block.instrs {
            let regions = instruction_regions_mut(instruction);
            if !regions.is_empty() {
                for region in regions {
                    folded += replace_folded(region, constant, reg_types, declarations);
                }
                continue;
            }
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            if let (Some(k), [dest]) = (
                instruction.folded_constant(constant, reg_types, declarations),
                results.as_slice(),
            ) {
                folded_here.push(*dest);
                *instruction = MirInstr::Const { dest: *dest, k };
                folded += 1;
            }
        }
        for instruction in &mut block.instrs {
            if let MirInstr::DefVar { src, in_place, .. } = instruction
                && folded_here.contains(src)
            {
                *in_place = false;
            }
        }
    }
    folded
}

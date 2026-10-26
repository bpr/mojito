//! Iterator-protocol instruction verification.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

pub(super) fn verify_iteration_instruction(
    cx: &InstrCx<'_>,
    instruction: &MirInstr,
    errors: &mut Vec<String>,
) {
    let prefix = cx.prefix;
    match instruction {
        MirInstr::GetIter { source, dest } => {
            if *source as usize >= cx.function.n_vars {
                errors.push(format!(
                    "{prefix}: GetIter uses invalid source slot {source}"
                ));
            }
            if *dest as usize >= cx.function.n_vars {
                errors.push(format!(
                    "{prefix}: GetIter uses invalid destination slot {dest}"
                ));
            }
        }
        MirInstr::Next { iter, .. } if *iter as usize >= cx.function.n_vars => {
            errors.push(format!("{prefix}: Next uses invalid iterator slot {iter}"));
        }
        _ => {}
    }
}

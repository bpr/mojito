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
        MirInstr::GetIter {
            source,
            dest,
            mode,
            prepare,
        } => {
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
            let Some(first) = prepare.first() else {
                return;
            };
            let convention_matches = |convention| match mode {
                mojito_checked::checked::IterationMode::Borrowed => matches!(
                    convention,
                    None | Some(
                        mojito_ast::ast::ArgConvention::Imm | mojito_ast::ast::ArgConvention::Ref
                    )
                ),
                mojito_checked::checked::IterationMode::Owned => {
                    convention == Some(mojito_ast::ast::ArgConvention::Var)
                }
            };
            if let Some(declaration) = declared(cx.declarations, first) {
                if !declaration.has_receiver || !declaration.param_types.is_empty() {
                    errors.push(format!(
                        "{prefix}: GetIter preparation method '{first}' is not a nullary receiver operation"
                    ));
                }
                if !convention_matches(declaration.receiver_convention) {
                    errors.push(format!(
                        "{prefix}: GetIter {mode:?} mode does not match preparation method '{first}' receiver convention {:?}",
                        declaration.receiver_convention
                    ));
                }
            } else {
                let matches_dispatch = match mode {
                    mojito_checked::checked::IterationMode::Borrowed => {
                        first
                            == &mojito_symbol::symbol::iterator_dispatch_symbol(
                                mojito_ast::ast::ArgConvention::Imm,
                            )
                            || first
                                == &mojito_symbol::symbol::iterator_dispatch_symbol(
                                    mojito_ast::ast::ArgConvention::Ref,
                                )
                    }
                    mojito_checked::checked::IterationMode::Owned => {
                        first
                            == &mojito_symbol::symbol::iterator_dispatch_symbol(
                                mojito_ast::ast::ArgConvention::Var,
                            )
                    }
                };
                if !matches_dispatch {
                    errors.push(format!(
                        "{prefix}: GetIter refers to undeclared preparation method '{first}'"
                    ));
                }
            }
        }
        MirInstr::Next { iter, .. } if *iter as usize >= cx.function.n_vars => {
            errors.push(format!("{prefix}: Next uses invalid iterator slot {iter}"));
        }
        _ => {}
    }
}

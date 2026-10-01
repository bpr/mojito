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
        MirInstr::Next { iter, .. } => {
            if *iter as usize >= cx.function.n_vars {
                errors.push(format!("{prefix}: Next uses invalid iterator slot {iter}"));
            }
        }
        MirInstr::TryNext {
            dest,
            yielded,
            iter,
            call,
            exhaustion,
        } => {
            if *iter as usize >= cx.function.n_vars {
                errors.push(format!(
                    "{prefix}: TryNext uses invalid iterator slot {iter}"
                ));
            }
            if cx.reg_ty(*yielded).is_some_and(|ty| *ty != Ty::Bool) {
                errors.push(format!(
                    "{prefix}: TryNext yielded flag has non-Bool type {}",
                    cx.reg_ty(*yielded).expect("checked above")
                ));
            }
            let is_stop_iteration = matches!(
                exhaustion,
                Ty::Struct(name, arguments)
                    if arguments.is_empty()
                        && (name == "StopIteration" || name.ends_with("$StopIteration"))
            );
            if !is_stop_iteration {
                errors.push(format!(
                    "{prefix}: TryNext catches non-StopIteration type {exhaustion}"
                ));
            }
            if cx.reg_ty(*dest) != Some(&call.result_ty) {
                errors.push(format!(
                    "{prefix}: TryNext result does not match its checked type {}",
                    call.result_ty
                ));
            }
            if call.raises.as_ref() != Some(exhaustion) {
                errors.push(format!(
                    "{prefix}: TryNext exhaustion type {exhaustion} does not match its checked call effect"
                ));
            }
            if verify_iterator_result_adapter(prefix, call, errors) {
                return;
            }
            match declared(cx.declarations, &call.target) {
                None => errors.push(format!(
                    "{prefix}: TryNext refers to undeclared iterator method '{}'",
                    call.target
                )),
                Some(declaration) => {
                    if !declaration.param_types.is_empty()
                        || declaration.receiver_convention
                            != Some(mojito_ast::ast::ArgConvention::Mut)
                    {
                        errors.push(format!(
                            "{prefix}: TryNext method '{}' is not a nullary 'mut self' operation",
                            call.target
                        ));
                    }
                    if !declaration.raises || declaration.error_ty.as_ref() != Some(exhaustion) {
                        errors.push(format!(
                            "{prefix}: TryNext exhaustion type {exhaustion} does not match '{}' raising contract",
                            call.target
                        ));
                    }
                    if !iterator_result_matches_declaration(call, declaration) {
                        errors.push(format!(
                            "{prefix}: TryNext result contract does not match '{}'",
                            call.target
                        ));
                    }
                }
            }
        }
        _ => {}
    }
}

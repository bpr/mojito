//! Structured control flow: the regions of `if`, `loop`, `comptime if`,
//! `comptime for`, and `try`, and the terminators that end them.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use crate::mir::text::{instruction_mnemonic, terminator_mnemonic};
use crate::mir::{LoopLabel, instruction_regions};
use mojito_types::types::TyKind;

/// [`verify_function_in`] for a structured body, upstream's `hlcf` form.
///
/// Every region, the body included, holds exactly one block, and no block
/// ends in a CFG terminator (`jump`, `branch`, `comptime_branch`,
/// `comptime_for`, `escape`). A `try` part still completes by `falloff`.
pub fn verify_structured_function_in(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
) -> Vec<String> {
    let mut errors = verify_function_in(name, function, declarations);
    if function.blocks.len() != 1 {
        errors.push(format!(
            "MIR function '{name}' body holds {} blocks",
            function.blocks.len()
        ));
    }
    structured_blocks(name, &function.blocks, &mut errors);
    errors
}

/// Verify each region a region op holds, under the region's kind and its
/// handler protection. A `try` body is protected by its handler; every other
/// region keeps its op's protection.
pub(super) fn verify_regions(cx: &InstrCx<'_>, instruction: &MirInstr, errors: &mut Vec<String>) {
    let protected = cx.context.protected;
    match instruction {
        MirInstr::Try { handler, .. } => {
            for (index, region) in instruction_regions(instruction).into_iter().enumerate() {
                let region_protected = protected || (index == 0 && handler.is_some());
                verify_region(cx, RegionKind::TryPart, region_protected, region, errors);
            }
        }
        MirInstr::If {
            results,
            cond,
            then,
            elifs,
            orelse,
        } => {
            require_bool(cx.prefix, cx.function, "if", *cond, errors);
            let arm = || RegionKind::IfArm(results.clone());
            verify_region(cx, arm(), protected, then, errors);
            for elif in elifs {
                verify_region(cx, RegionKind::ElifCond, protected, &elif.cond, errors);
                verify_region(cx, arm(), protected, &elif.then, errors);
            }
            verify_region(cx, arm(), protected, orelse, errors);
        }
        MirInstr::Loop { label, body } => {
            verify_region(cx, RegionKind::LoopBody(*label), protected, body, errors);
        }
        MirInstr::ComptimeIf {
            results,
            conds,
            arms,
            orelse,
        } => {
            if conds.is_empty() || conds.len() != arms.len() {
                errors.push(format!(
                    "{}: comptime if has {} conditions for {} arms",
                    cx.prefix,
                    conds.len(),
                    arms.len()
                ));
            }
            for region in arms.iter().chain(std::iter::once(orelse)) {
                let kind = RegionKind::ComptimeIfArm(results.clone());
                verify_region(cx, kind, protected, region, errors);
            }
        }
        MirInstr::ComptimeFor {
            slot, body, orelse, ..
        } => {
            if *slot as usize >= cx.function.n_vars {
                errors.push(format!(
                    "{}: compile-time loop slot {slot} is out of range",
                    cx.prefix
                ));
            }
            let unreachable_else = matches!(
                orelse.as_slice(),
                [MirBlock {
                    instrs,
                    term: MirTerm::Unreachable,
                }] if instrs.is_empty()
            );
            if !unreachable_else {
                errors.push(format!(
                    "{}: comptime for else region is not one `unreachable` block",
                    cx.prefix
                ));
            }
            verify_region(cx, RegionKind::ComptimeForBody, protected, body, errors);
            verify_region(cx, RegionKind::ComptimeForElse, protected, orelse, errors);
        }
        _ => {}
    }
}

/// A region terminator sits in the region it ends: `yield` in an `if` arm
/// and `comptime.yield` in a `comptime if` arm, each with its op's results;
/// `if.elifcond.yield` in an elif condition, with a `Bool`; `break` and
/// `continue` inside the loop they name (the nearest, or the nearest
/// carrying the label); `comptime.for.break`/`continue` where the nearest
/// enclosing loop is a `comptime for`. `unreachable` ends any region.
pub(super) fn verify_region_terminator(
    prefix: &str,
    function: &MirFunction,
    terminator: &MirTerm,
    context: &RegionContext,
    errors: &mut Vec<String>,
) {
    let mnemonic = terminator_mnemonic(terminator);
    match (terminator, context.kind()) {
        (MirTerm::Yield(values), RegionKind::IfArm(results))
        | (MirTerm::ComptimeYield(values), RegionKind::ComptimeIfArm(results)) => {
            yielded_values(prefix, function, mnemonic, values, results, errors);
        }
        (MirTerm::ElifCond(cond), RegionKind::ElifCond) => {
            require_bool(prefix, function, mnemonic, *cond, errors);
        }
        (MirTerm::Yield(_) | MirTerm::ComptimeYield(_) | MirTerm::ElifCond(_), _) => {
            errors.push(format!("{prefix}: `{mnemonic}` outside the region it ends"));
        }
        (MirTerm::Break { label } | MirTerm::Continue { label }, _) => {
            if !has_enclosing_loop(context, *label) {
                let named =
                    label.map_or_else(String::new, |label| format!(" labelled loop{}", label.0));
                errors.push(format!(
                    "{prefix}: `{mnemonic}` has no enclosing loop{named}"
                ));
            }
        }
        (MirTerm::ComptimeBreak | MirTerm::ComptimeContinue, _) => {
            let nearest =
                context.regions.iter().rev().find(|kind| {
                    matches!(kind, RegionKind::LoopBody(_) | RegionKind::ComptimeForBody)
                });
            if !matches!(nearest, Some(RegionKind::ComptimeForBody)) {
                errors.push(format!(
                    "{prefix}: `{mnemonic}` is not inside a comptime for body"
                ));
            }
        }
        _ => {}
    }
}

fn verify_region(
    cx: &InstrCx<'_>,
    kind: RegionKind,
    protected: bool,
    blocks: &[MirBlock],
    errors: &mut Vec<String>,
) {
    if blocks.is_empty() {
        errors.push(format!("{}: region holds no block", cx.prefix));
    }
    let mut regions = cx.context.regions.clone();
    regions.push(kind);
    let context = RegionContext {
        region_len: blocks.len(),
        function_len: cx.context.function_len,
        regions,
        protected,
    };
    verify_blocks(
        cx.name,
        cx.function,
        cx.declarations,
        blocks,
        &context,
        errors,
    );
}

fn require_bool(
    prefix: &str,
    function: &MirFunction,
    what: &str,
    cond: Reg,
    errors: &mut Vec<String>,
) {
    if let Some(found) = function.reg_types.get(&cond.0)
        && *found != TyKind::Bool
    {
        errors.push(format!("{prefix}: `{what}` condition has type {found}"));
    }
}

fn yielded_values(
    prefix: &str,
    function: &MirFunction,
    mnemonic: &str,
    values: &[Reg],
    results: &[Reg],
    errors: &mut Vec<String>,
) {
    if values.len() != results.len() {
        errors.push(format!(
            "{prefix}: `{mnemonic}` yields {} values for {} results",
            values.len(),
            results.len()
        ));
        return;
    }
    for (value, result) in values.iter().zip(results) {
        if let (Some(found), Some(expected)) = (
            function.reg_types.get(&value.0),
            function.reg_types.get(&result.0),
        ) && !types_compatible(found, expected)
        {
            errors.push(format!(
                "{prefix}: `{mnemonic}` yields {found} for a result of type {expected}"
            ));
        }
    }
}

/// Whether a `break`/`continue` with `label` names a loop enclosing it: the
/// nearest loop when unlabelled, else the nearest carrying the label.
fn has_enclosing_loop(context: &RegionContext, label: Option<LoopLabel>) -> bool {
    context.regions.iter().any(|kind| match kind {
        RegionKind::LoopBody(found) => label.is_none() || *found == label,
        _ => false,
    })
}

fn structured_blocks(name: &str, blocks: &[MirBlock], errors: &mut Vec<String>) {
    for (index, block) in blocks.iter().enumerate() {
        let prefix = format!("MIR function '{name}' block {index}");
        if matches!(
            block.term,
            MirTerm::Jump(_)
                | MirTerm::Branch { .. }
                | MirTerm::ComptimeBranch { .. }
                | MirTerm::ComptimeFor { .. }
                | MirTerm::EscapeJump { .. }
        ) {
            errors.push(format!(
                "{prefix}: CFG terminator `{}` in a structured body",
                terminator_mnemonic(&block.term)
            ));
        }
        for instruction in &block.instrs {
            for region in instruction_regions(instruction) {
                if region.len() != 1 {
                    errors.push(format!(
                        "{prefix}: `{}` region holds {} blocks",
                        instruction_mnemonic(instruction),
                        region.len()
                    ));
                }
                structured_blocks(name, region, errors);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::{MirProgram, SpanTable};
    use mojito_checked::checked::ComptimeSequence;
    use mojito_types::param_expr::{ParamContext, ParamId, ParamRef};

    fn one(instrs: Vec<MirInstr>, term: MirTerm) -> Vec<MirBlock> {
        vec![MirBlock { instrs, term }]
    }

    /// A body of `instrs` over a `Bool` and two `Int` registers and one
    /// `Int` slot.
    fn function_with(instrs: Vec<MirInstr>) -> MirFunction {
        MirFunction {
            blocks: one(instrs, MirTerm::Return(None)),
            n_regs: 3,
            n_vars: 1,
            var_names: vec!["i".into()],
            var_tys: HashMap::from([(0, Ty::int())]),
            ret_ty: Some(Ty::none()),
            spans: SpanTable(HashMap::new()),
            reg_types: HashMap::from([(0, Ty::bool()), (1, Ty::int()), (2, Ty::int())]),
            ..MirFunction::default()
        }
    }

    fn findings(function: &MirFunction) -> Vec<String> {
        verify_function_in("f", function, &MirDeclarations::default())
    }

    fn assert_finding(found: &[String], expected: &str) {
        assert!(
            found.iter().any(|finding| finding.contains(expected)),
            "no finding `{expected}` in {found:?}"
        );
    }

    fn comptime_for(body: Vec<MirBlock>) -> MirInstr {
        let context = ParamContext::detached();
        MirInstr::ComptimeFor {
            binder: ParamRef {
                id: ParamId::new("$comptime_for@f:0..1", 0),
                name: "i".into(),
            },
            slot: 0,
            source: ComptimeSequence::Elements(
                context
                    .constant(CtValue::List(vec![CtValue::Int(0), CtValue::Int(1)]))
                    .expect("a constant"),
            ),
            body,
            orelse: one(Vec::new(), MirTerm::Unreachable),
        }
    }

    #[test]
    fn a_well_formed_if_with_results_verifies() {
        let function = function_with(vec![MirInstr::If {
            results: vec![Reg(1)],
            cond: Reg(0),
            then: one(Vec::new(), MirTerm::Yield(vec![Reg(2)])),
            elifs: vec![crate::mir::MirElif {
                cond: one(Vec::new(), MirTerm::ElifCond(Reg(0))),
                then: one(Vec::new(), MirTerm::Yield(vec![Reg(2)])),
            }],
            orelse: one(Vec::new(), MirTerm::Unreachable),
        }]);
        assert_eq!(findings(&function), Vec::<String>::new());
    }

    #[test]
    fn a_region_terminator_outside_its_region_is_a_finding() {
        let misplaced = function_with(vec![MirInstr::Loop {
            label: None,
            body: one(Vec::new(), MirTerm::Yield(Vec::new())),
        }]);
        assert_finding(&findings(&misplaced), "`yield` outside the region it ends");
        let elif = function_with(vec![MirInstr::If {
            results: Vec::new(),
            cond: Reg(0),
            then: one(Vec::new(), MirTerm::ElifCond(Reg(0))),
            elifs: Vec::new(),
            orelse: one(Vec::new(), MirTerm::ComptimeYield(Vec::new())),
        }]);
        let found = findings(&elif);
        assert_finding(&found, "`if.elifcond.yield` outside the region it ends");
        assert_finding(&found, "`comptime.yield` outside the region it ends");
        let arity = function_with(vec![MirInstr::If {
            results: vec![Reg(1)],
            cond: Reg(0),
            then: one(Vec::new(), MirTerm::Yield(Vec::new())),
            elifs: Vec::new(),
            orelse: one(Vec::new(), MirTerm::Yield(vec![Reg(0)])),
        }]);
        let found = findings(&arity);
        assert_finding(&found, "`yield` yields 0 values for 1 results");
        assert_finding(&found, "`yield` yields Bool for a result of type Int");
    }

    #[test]
    fn a_break_names_an_enclosing_loop() {
        let unlabelled = function_with(vec![MirInstr::If {
            results: Vec::new(),
            cond: Reg(0),
            then: one(Vec::new(), MirTerm::Break { label: None }),
            elifs: Vec::new(),
            orelse: one(Vec::new(), MirTerm::Yield(Vec::new())),
        }]);
        assert_finding(&findings(&unlabelled), "`break` has no enclosing loop");
        let labelled = function_with(vec![MirInstr::Loop {
            label: Some(LoopLabel(1)),
            body: one(
                Vec::new(),
                MirTerm::Continue {
                    label: Some(LoopLabel(2)),
                },
            ),
        }]);
        assert_finding(
            &findings(&labelled),
            "`continue` has no enclosing loop labelled loop2",
        );
        let named = function_with(vec![MirInstr::Loop {
            label: Some(LoopLabel(1)),
            body: one(
                vec![MirInstr::Loop {
                    label: None,
                    body: one(
                        Vec::new(),
                        MirTerm::Break {
                            label: Some(LoopLabel(1)),
                        },
                    ),
                }],
                MirTerm::Continue { label: None },
            ),
        }]);
        assert_eq!(findings(&named), Vec::<String>::new());
    }

    #[test]
    fn a_comptime_break_needs_the_nearest_loop_to_be_a_comptime_for() {
        let through_loop = function_with(vec![comptime_for(one(
            vec![MirInstr::Loop {
                label: None,
                body: one(Vec::new(), MirTerm::ComptimeBreak),
            }],
            MirTerm::ComptimeContinue,
        ))]);
        let found = findings(&through_loop);
        assert_finding(
            &found,
            "`comptime.for.break` is not inside a comptime for body",
        );
        assert_eq!(found.len(), 1, "{found:?}");
    }

    #[test]
    fn a_multi_block_region_is_rejected_only_by_the_structured_mode() {
        let function = function_with(vec![MirInstr::Loop {
            label: None,
            body: vec![
                MirBlock {
                    instrs: Vec::new(),
                    term: MirTerm::Jump(1),
                },
                MirBlock {
                    instrs: Vec::new(),
                    term: MirTerm::Break { label: None },
                },
            ],
        }]);
        assert_eq!(findings(&function), Vec::<String>::new());
        let structured = verify_structured_function_in("f", &function, &MirDeclarations::default());
        assert_finding(&structured, "`loop` region holds 2 blocks");
        assert_finding(&structured, "CFG terminator `jump` in a structured body");
    }

    #[test]
    fn concrete_mir_carries_no_structured_control_flow() {
        let function = function_with(vec![MirInstr::If {
            results: Vec::new(),
            cond: Reg(0),
            then: one(Vec::new(), MirTerm::Yield(Vec::new())),
            elifs: Vec::new(),
            orelse: one(Vec::new(), MirTerm::Yield(Vec::new())),
        }]);
        let program = MirProgram {
            functions: vec![("f".into(), function)],
            declarations: MirDeclarations::default(),
            invariant_errors: Vec::new(),
        };
        let found = verify_concrete(&program);
        assert_finding(
            &found,
            "keeps structured control flow `if` in elaborated MIR",
        );
        assert_finding(
            &found,
            "keeps the region terminator `yield` in elaborated MIR",
        );
    }
}

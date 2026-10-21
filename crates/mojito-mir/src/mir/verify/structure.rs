//! Structured control flow: the regions of `if`, `loop`, `comptime if`,
//! `comptime for`, `match`, and `try`, and the terminators that end them.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use crate::mir::text::{instruction_mnemonic, terminator_mnemonic};
use crate::mir::{LoopLabel, instruction_regions};
use mojito_types::types::TyKind;

/// [`verify_function_in`] for a structured body, upstream's `hlcf` form,
/// plus its [`verify_structured_shape`].
pub fn verify_structured_function_in(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
) -> Vec<String> {
    let mut errors = verify_function_in(name, function, declarations);
    errors.extend(verify_structured_shape(name, function));
    errors
}

/// The shape of a structured body: every region, the body included, holds
/// exactly one block, and no block ends in a CFG terminator (`jump`,
/// `branch`, `comptime_branch`, `comptime_for`, `escape`).
pub fn verify_structured_shape(name: &str, function: &MirFunction) -> Vec<String> {
    let mut errors = Vec::new();
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
        MirInstr::Loop {
            label,
            results,
            body,
        } => {
            let kind = RegionKind::LoopBody {
                label: *label,
                results: results.clone(),
            };
            verify_region(cx, kind, protected, body, errors);
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
            source,
            body,
            orelse,
            ..
        } => {
            let tests_has_next = matches!(
                body.as_slice(),
                [MirBlock {
                    instrs,
                    term: MirTerm::Unreachable,
                }] if matches!(
                    instrs.first(),
                    Some(MirInstr::ComptimeIf { conds, .. }) if conds == &[source.stop_test()]
                )
            );
            if !tests_has_next {
                errors.push(format!(
                    "{}: comptime for body does not open with its has_next test",
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
        MirInstr::Match { cases, orelse } => {
            if cases.is_empty() {
                errors.push(format!("{}: match holds no case", cx.prefix));
            }
            for case in cases {
                verify_region(cx, RegionKind::MatchCase, protected, case, errors);
            }
            verify_region(cx, RegionKind::IfArm(Vec::new()), protected, orelse, errors);
        }
        _ => {}
    }
}

/// A region terminator sits in the region it ends: `yield` in an `if` arm
/// and `comptime.yield` in a `comptime if` arm, each with its op's results;
/// `if.elifcond.yield` in an elif condition, with a `Bool`; `break` and
/// `continue` inside the loop they name (the nearest, or the nearest
/// carrying the label); `comptime.for.break`/`continue` where the nearest
/// enclosing loop is a `comptime for`; `match.next`/`match.complete` inside
/// a `match` case. `unreachable` ends any region.
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
        (MirTerm::Break { label, .. } | MirTerm::Continue { label }, _) => {
            match (enclosing_loop(context, *label), terminator) {
                (None, _) => {
                    let named = label
                        .map_or_else(String::new, |label| format!(" labelled loop{}", label.0));
                    errors.push(format!(
                        "{prefix}: `{mnemonic}` has no enclosing loop{named}"
                    ));
                }
                (Some(results), MirTerm::Break { values, .. }) => {
                    yielded_values(prefix, function, mnemonic, values, results, errors);
                }
                (Some(_), _) => {}
            }
        }
        (MirTerm::ComptimeBreak | MirTerm::ComptimeContinue, _) => {
            let nearest = context.regions.iter().rev().find(|kind| {
                matches!(
                    kind,
                    RegionKind::LoopBody { .. } | RegionKind::ComptimeForBody
                )
            });
            if !matches!(nearest, Some(RegionKind::ComptimeForBody)) {
                errors.push(format!(
                    "{prefix}: `{mnemonic}` is not inside a comptime for body"
                ));
            }
        }
        (MirTerm::MatchNext | MirTerm::MatchComplete, _)
            if !context
                .regions
                .iter()
                .any(|kind| matches!(kind, RegionKind::MatchCase)) =>
        {
            errors.push(format!("{prefix}: `{mnemonic}` is not inside a match case"));
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

/// The results of the loop a `break`/`continue` with `label` names: the
/// nearest enclosing loop when unlabelled, else the nearest carrying the
/// label; `None` when no enclosing loop matches.
fn enclosing_loop(context: &RegionContext, label: Option<LoopLabel>) -> Option<&[Reg]> {
    context.regions.iter().rev().find_map(|kind| match kind {
        RegionKind::LoopBody {
            label: found,
            results,
        } if label.is_none() || *found == label => Some(results.as_slice()),
        _ => None,
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

    /// A `comptime for` whose body opens with its `has_next` test, the
    /// test's arm `suite`.
    fn comptime_for(suite: Vec<MirBlock>) -> MirInstr {
        let source = sequence();
        let test = MirInstr::ComptimeIf {
            results: Vec::new(),
            conds: vec![source.stop_test()],
            arms: vec![suite],
            orelse: one(Vec::new(), MirTerm::ComptimeBreak),
        };
        bare_comptime_for(source, one(vec![test], MirTerm::Unreachable))
    }

    fn bare_comptime_for(source: ComptimeSequence, body: Vec<MirBlock>) -> MirInstr {
        MirInstr::ComptimeFor {
            binder: ParamRef {
                id: ParamId::new("$comptime_for@f:0..1", 0),
                name: "i".into(),
            },
            source,
            body,
            orelse: one(Vec::new(), MirTerm::Unreachable),
        }
    }

    fn sequence() -> ComptimeSequence {
        let context = ParamContext::detached();
        let constant = |value| context.constant(value).expect("a constant");
        ComptimeSequence {
            initial: constant(CtValue::Int(0)),
            has_next: constant(CtValue::Bool(false)),
            next_iter: constant(CtValue::Int(1)),
            next_value: constant(CtValue::Int(0)),
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
            results: Vec::new(),
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

    /// A loop labelled `label` with `results`, whose body breaks `inner`
    /// with `values` from the then arm of an `if` on `r0`.
    fn valued_loop(
        label: Option<LoopLabel>,
        results: Vec<Reg>,
        inner: Option<LoopLabel>,
        values: Vec<Reg>,
    ) -> MirInstr {
        MirInstr::Loop {
            label,
            results,
            body: one(
                vec![MirInstr::If {
                    results: Vec::new(),
                    cond: Reg(0),
                    then: one(
                        Vec::new(),
                        MirTerm::Break {
                            label: inner,
                            values,
                        },
                    ),
                    elifs: Vec::new(),
                    orelse: one(Vec::new(), MirTerm::Yield(Vec::new())),
                }],
                MirTerm::Continue { label: None },
            ),
        }
    }

    #[test]
    fn a_break_with_values_verifies() {
        let function = function_with(vec![valued_loop(None, vec![Reg(1)], None, vec![Reg(2)])]);
        assert_eq!(findings(&function), Vec::<String>::new());
    }

    #[test]
    fn a_break_carrying_the_wrong_values_is_a_finding() {
        let arity = function_with(vec![valued_loop(None, vec![Reg(1)], None, Vec::new())]);
        assert_finding(&findings(&arity), "`break` yields 0 values for 1 results");
        let typed = function_with(vec![valued_loop(None, vec![Reg(1)], None, vec![Reg(0)])]);
        assert_finding(
            &findings(&typed),
            "`break` yields Bool for a result of type Int",
        );
        // A labelled break is checked against the loop it names, not the
        // nearest one.
        let outer = Some(LoopLabel(7));
        let nested = function_with(vec![MirInstr::Loop {
            label: outer,
            results: vec![Reg(1)],
            body: one(
                vec![valued_loop(None, Vec::new(), outer, Vec::new())],
                MirTerm::Continue { label: None },
            ),
        }]);
        let found = findings(&nested);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_finding(&found, "`break` yields 0 values for 1 results");
    }

    /// A `match` of two cases over a `Bool` subject: the first tests `r0`
    /// and advances on failure, the second always completes; a nested
    /// `if` in a case may leave it through either terminator.
    fn two_case_match() -> MirInstr {
        let test = MirInstr::If {
            results: Vec::new(),
            cond: Reg(0),
            then: one(Vec::new(), MirTerm::Yield(Vec::new())),
            elifs: Vec::new(),
            orelse: one(Vec::new(), MirTerm::MatchNext),
        };
        MirInstr::Match {
            cases: vec![
                one(vec![test], MirTerm::MatchComplete),
                one(Vec::new(), MirTerm::MatchComplete),
            ],
            orelse: one(Vec::new(), MirTerm::Yield(Vec::new())),
        }
    }

    #[test]
    fn a_well_formed_match_verifies() {
        let function = function_with(vec![two_case_match()]);
        assert_eq!(findings(&function), Vec::<String>::new());
        // A case may be left from a loop nested in it, and a loop in a case
        // keeps its own `break`.
        let nested = function_with(vec![MirInstr::Match {
            cases: vec![one(
                vec![MirInstr::Loop {
                    label: None,
                    results: Vec::new(),
                    body: one(
                        vec![MirInstr::If {
                            results: Vec::new(),
                            cond: Reg(0),
                            then: one(Vec::new(), MirTerm::MatchComplete),
                            elifs: Vec::new(),
                            orelse: one(
                                Vec::new(),
                                MirTerm::Break {
                                    label: None,
                                    values: Vec::new(),
                                },
                            ),
                        }],
                        MirTerm::Continue { label: None },
                    ),
                }],
                MirTerm::MatchNext,
            )],
            orelse: one(Vec::new(), MirTerm::Yield(Vec::new())),
        }]);
        assert_eq!(findings(&nested), Vec::<String>::new());
    }

    #[test]
    fn a_match_terminator_outside_a_case_is_a_finding() {
        let next = function_with(vec![MirInstr::If {
            results: Vec::new(),
            cond: Reg(0),
            then: one(Vec::new(), MirTerm::MatchNext),
            elifs: Vec::new(),
            orelse: one(Vec::new(), MirTerm::MatchComplete),
        }]);
        let found = findings(&next);
        assert_finding(&found, "`match.next` is not inside a match case");
        assert_finding(&found, "`match.complete` is not inside a match case");
        // The else region is an arm, not a case.
        let orelse = function_with(vec![MirInstr::Match {
            cases: vec![one(Vec::new(), MirTerm::MatchComplete)],
            orelse: one(Vec::new(), MirTerm::MatchNext),
        }]);
        assert_finding(
            &findings(&orelse),
            "`match.next` is not inside a match case",
        );
        let empty = function_with(vec![MirInstr::Match {
            cases: Vec::new(),
            orelse: one(Vec::new(), MirTerm::Yield(Vec::new())),
        }]);
        assert_finding(&findings(&empty), "match holds no case");
    }

    #[test]
    fn a_break_names_an_enclosing_loop() {
        let unlabelled = function_with(vec![MirInstr::If {
            results: Vec::new(),
            cond: Reg(0),
            then: one(
                Vec::new(),
                MirTerm::Break {
                    label: None,
                    values: Vec::new(),
                },
            ),
            elifs: Vec::new(),
            orelse: one(Vec::new(), MirTerm::Yield(Vec::new())),
        }]);
        assert_finding(&findings(&unlabelled), "`break` has no enclosing loop");
        let labelled = function_with(vec![MirInstr::Loop {
            label: Some(LoopLabel(1)),
            results: Vec::new(),
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
            results: Vec::new(),
            body: one(
                vec![MirInstr::Loop {
                    label: None,
                    results: Vec::new(),
                    body: one(
                        Vec::new(),
                        MirTerm::Break {
                            label: Some(LoopLabel(1)),
                            values: Vec::new(),
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
                results: Vec::new(),
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
    fn a_comptime_for_body_opens_with_its_has_next_test() {
        let tested = function_with(vec![comptime_for(one(
            Vec::new(),
            MirTerm::ComptimeContinue,
        ))]);
        assert_eq!(findings(&tested), Vec::<String>::new());
        let bare = function_with(vec![bare_comptime_for(
            sequence(),
            one(Vec::new(), MirTerm::ComptimeContinue),
        )]);
        assert_finding(
            &findings(&bare),
            "comptime for body does not open with its has_next test",
        );
    }

    #[test]
    fn a_multi_block_region_is_rejected_only_by_the_structured_mode() {
        let function = function_with(vec![MirInstr::Loop {
            label: None,
            results: Vec::new(),
            body: vec![
                MirBlock {
                    instrs: Vec::new(),
                    term: MirTerm::Jump(1),
                },
                MirBlock {
                    instrs: Vec::new(),
                    term: MirTerm::Break {
                        label: None,
                        values: Vec::new(),
                    },
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

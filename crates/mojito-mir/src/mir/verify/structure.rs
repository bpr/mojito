//! Structured control flow: the regions of `if`, `loop`, `comptime if`,
//! `comptime for`, `match`, and `try`, and the terminators that end them.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use crate::mir::LoopLabel;
use crate::mir::text::{instruction_mnemonic, terminator_mnemonic};
use mojito_types::types::TyKind;

/// Verify each region a region op holds, under the region's kind and its
/// handler protection. A `try` body is protected by its handler; every other
/// region keeps its op's protection.
pub(super) fn verify_regions(cx: &InstrCx<'_>, instruction: &MirInstr, errors: &mut Vec<String>) {
    let protected = cx.context.protected;
    match instruction {
        MirInstr::Try {
            label,
            results,
            body,
            handler_args,
            handler,
            else_args,
            orelse,
            ..
        } => {
            for (args, region, part) in [
                (handler_args, handler.is_some(), "handler"),
                (else_args, orelse.is_some(), "else"),
            ] {
                if !region && !args.is_empty() {
                    errors.push(format!(
                        "{}: `try` has {} {part} arguments but no {part}",
                        cx.prefix,
                        args.len()
                    ));
                }
            }
            let kind = RegionKind::TryBody {
                label: *label,
                handled: handler.as_ref().map(|_| handler_args.clone()),
                completes: if orelse.is_some() {
                    else_args.clone()
                } else {
                    results.clone()
                },
                into_else: orelse.is_some(),
            };
            verify_region(cx, kind, protected || handler.is_some(), body, errors);
            for region in handler.iter().chain(orelse) {
                let kind = RegionKind::TryPart(results.clone());
                verify_region(cx, kind, protected, region, errors);
            }
        }
        MirInstr::If {
            results,
            cond,
            then,
            elifs,
            else_args,
            orelse,
        } => {
            require_bool(cx.prefix, cx.function, "if", *cond, errors);
            let entered = elifs.first().map_or(else_args, |elif| &elif.cond_args);
            let target = YieldTarget::Arguments;
            yielded_values(cx.prefix, cx.function, "if", &[], entered, target, errors);
            let arm = || RegionKind::IfArm(results.clone());
            verify_region(cx, arm(), protected, then, errors);
            for (index, elif) in elifs.iter().enumerate() {
                let next_args = elifs
                    .get(index + 1)
                    .map_or(else_args, |next| &next.cond_args);
                let kind = RegionKind::ElifCond {
                    then_args: elif.then_args.clone(),
                    next_args: next_args.clone(),
                };
                verify_region(cx, kind, protected, &elif.cond, errors);
                verify_region(cx, arm(), protected, &elif.then, errors);
            }
            verify_region(cx, arm(), protected, orelse, errors);
        }
        MirInstr::Loop {
            label,
            operands,
            args,
            results,
            body,
        } => {
            let mnemonic = instruction_mnemonic(instruction);
            let target = YieldTarget::Arguments;
            yielded_values(
                cx.prefix,
                cx.function,
                mnemonic,
                operands,
                args,
                target,
                errors,
            );
            let kind = RegionKind::LoopBody {
                label: *label,
                args: args.clone(),
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
            operands,
            args,
            results,
            body,
            orelse,
            ..
        } => {
            comptime_for_values(cx, operands, args, results, errors);
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
            let kind = RegionKind::ComptimeForBody {
                args: args.clone(),
                results: results.clone(),
            };
            verify_region(cx, kind, protected, body, errors);
            verify_region(cx, RegionKind::ComptimeForElse, protected, orelse, errors);
        }
        MirInstr::Match {
            results,
            cases,
            else_args,
            orelse,
        } => {
            match cases.first() {
                None => errors.push(format!("{}: match holds no case", cx.prefix)),
                Some(first) => {
                    let target = YieldTarget::Arguments;
                    let args = &first.args;
                    yielded_values(cx.prefix, cx.function, "match", &[], args, target, errors);
                }
            }
            for (index, case) in cases.iter().enumerate() {
                let next_args = cases.get(index + 1).map_or(else_args, |next| &next.args);
                let kind = RegionKind::MatchCase {
                    next_args: next_args.clone(),
                    results: results.clone(),
                };
                verify_region(cx, kind, protected, &case.body, errors);
            }
            verify_region(
                cx,
                RegionKind::IfArm(results.clone()),
                protected,
                orelse,
                errors,
            );
        }
        _ => {}
    }
}

/// A region terminator sits in the region it ends: `yield` in an `if` arm
/// and `comptime.yield` in a `comptime if` arm, each with its op's results;
/// `if.elifcond.yield` in an elif condition, with a `Bool` and values for
/// its arm's arguments and the next region's; `break` and `continue` inside
/// the loop they name (the nearest, or the nearest carrying the label);
/// `comptime.for.break`/`continue` where the nearest enclosing loop is a
/// `comptime for`, with values for its results or its body's arguments;
/// `match.next`/`match.complete` inside a `match` case, a `match.next` with
/// values for the next region's arguments and a `match.complete` with the
/// op's results; a `match`'s else region's `yield` with those results too;
/// `try.yield` in a `try` part, a body's with the `else`'s arguments (the
/// op's results without one) and a handler's or `else`'s with the op's
/// results; a labelled `raise` inside the body of the `try` it names, with
/// values for its handler's arguments, and an unlabelled one with none.
/// `unreachable`
/// ends any region. Values are checked against each region they enter, as
/// upstream's `verifyTerminator` checks each target, stopping at the first
/// finding.
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
            let target = YieldTarget::Results;
            yielded_values(prefix, function, mnemonic, values, results, target, errors);
        }
        (
            MirTerm::ElifCond { cond, values },
            RegionKind::ElifCond {
                then_args,
                next_args,
            },
        ) => {
            require_bool(prefix, function, mnemonic, *cond, errors);
            let target = YieldTarget::Arguments;
            let found = errors.len();
            yielded_values(
                prefix, function, mnemonic, values, then_args, target, errors,
            );
            if errors.len() == found {
                yielded_values(
                    prefix, function, mnemonic, values, next_args, target, errors,
                );
            }
        }
        (
            MirTerm::TryYield(values),
            RegionKind::TryBody {
                completes,
                into_else,
                ..
            },
        ) => {
            let target = if *into_else {
                YieldTarget::Arguments
            } else {
                YieldTarget::Results
            };
            yielded_values(
                prefix, function, mnemonic, values, completes, target, errors,
            );
        }
        (MirTerm::TryYield(values), RegionKind::TryPart(results)) => {
            let target = YieldTarget::Results;
            yielded_values(prefix, function, mnemonic, values, results, target, errors);
        }
        (MirTerm::ErrorReturn { .. }, _) => {}
        (MirTerm::TryRaise { label, values }, _) => match context.raise_target(*label) {
            None => errors.push(format!(
                "{prefix}: `{mnemonic}` names try{}, which no enclosing try body carries",
                label.0
            )),
            Some(None) => errors.push(format!(
                "{prefix}: `{mnemonic}` names try{}, which has no handler",
                label.0
            )),
            Some(Some(args)) => {
                let target = YieldTarget::Arguments;
                yielded_values(prefix, function, mnemonic, values, args, target, errors);
            }
        },
        (
            MirTerm::Yield(_)
            | MirTerm::ComptimeYield(_)
            | MirTerm::ElifCond { .. }
            | MirTerm::TryYield(_),
            _,
        ) => {
            errors.push(format!("{prefix}: `{mnemonic}` outside the region it ends"));
        }
        (MirTerm::Break { label, values } | MirTerm::Continue { label, values }, _) => {
            match enclosing_loop(context, *label) {
                None => {
                    let named = label
                        .map_or_else(String::new, |label| format!(" labelled loop{}", label.0));
                    errors.push(format!(
                        "{prefix}: `{mnemonic}` has no enclosing loop{named}"
                    ));
                }
                Some((args, results)) => {
                    let (expected, target) = if matches!(terminator, MirTerm::Continue { .. }) {
                        (args, YieldTarget::Arguments)
                    } else {
                        (results, YieldTarget::Results)
                    };
                    yielded_values(prefix, function, mnemonic, values, expected, target, errors);
                }
            }
        }
        (MirTerm::ComptimeBreak(values) | MirTerm::ComptimeContinue(values), _) => {
            let nearest = context.regions.iter().rev().find(|kind| {
                matches!(
                    kind,
                    RegionKind::LoopBody { .. } | RegionKind::ComptimeForBody { .. }
                )
            });
            match nearest {
                Some(RegionKind::ComptimeForBody { args, results }) => {
                    let (expected, target) = if matches!(terminator, MirTerm::ComptimeContinue(_)) {
                        (args, YieldTarget::Arguments)
                    } else {
                        (results, YieldTarget::Results)
                    };
                    yielded_values(prefix, function, mnemonic, values, expected, target, errors);
                }
                _ => errors.push(format!(
                    "{prefix}: `{mnemonic}` is not inside a comptime for body"
                )),
            }
        }
        (MirTerm::MatchNext(values) | MirTerm::MatchComplete(values), _) => {
            match enclosing_case(context) {
                None => errors.push(format!("{prefix}: `{mnemonic}` is not inside a match case")),
                Some((next_args, results)) => {
                    let (expected, target) = if matches!(terminator, MirTerm::MatchNext(_)) {
                        (next_args, YieldTarget::Arguments)
                    } else {
                        (results, YieldTarget::Results)
                    };
                    yielded_values(prefix, function, mnemonic, values, expected, target, errors);
                }
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
    if blocks.len() != 1 {
        errors.push(format!(
            "{}: region holds {} blocks",
            cx.prefix,
            blocks.len()
        ));
    }
    let mut regions = cx.context.regions.clone();
    regions.push(kind);
    let context = RegionContext { regions, protected };
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

/// What the values along a control-flow edge define: a region op's
/// results, or a loop body's arguments.
#[derive(Clone, Copy)]
enum YieldTarget {
    Results,
    Arguments,
}

/// The values along one edge match what they define in number and type,
/// upstream's `verifyTypesAlongEdge`.
fn yielded_values(
    prefix: &str,
    function: &MirFunction,
    mnemonic: &str,
    values: &[Reg],
    targets: &[Reg],
    target: YieldTarget,
    errors: &mut Vec<String>,
) {
    let (one, many) = match target {
        YieldTarget::Results => ("a result", "results"),
        YieldTarget::Arguments => ("an argument", "arguments"),
    };
    if values.len() != targets.len() {
        errors.push(format!(
            "{prefix}: `{mnemonic}` yields {} values for {} {many}",
            values.len(),
            targets.len()
        ));
        return;
    }
    for (value, defined) in values.iter().zip(targets) {
        if let (Some(found), Some(expected)) = (
            function.reg_types.get(&value.0),
            function.reg_types.get(&defined.0),
        ) && !types_compatible(found, expected)
        {
            errors.push(format!(
                "{prefix}: `{mnemonic}` yields {found} for {one} of type {expected}"
            ));
        }
    }
}

/// The arguments of the region a `match.next` enters and the results a
/// `match.complete` defines: the nearest enclosing `match` case's
/// `next_args` and its op's `results`; `None` outside every case.
fn enclosing_case(context: &RegionContext) -> Option<(&[Reg], &[Reg])> {
    context.regions.iter().rev().find_map(|kind| match kind {
        RegionKind::MatchCase { next_args, results } => {
            Some((next_args.as_slice(), results.as_slice()))
        }
        _ => None,
    })
}

/// A `comptime for`'s loop-carried values: its operands and results agree
/// in number and type, as `ComptimeForOp::verify` requires, and the
/// operands enter its body's arguments.
fn comptime_for_values(
    cx: &InstrCx<'_>,
    operands: &[Reg],
    args: &[Reg],
    results: &[Reg],
    errors: &mut Vec<String>,
) {
    if operands.len() != results.len() {
        errors.push(format!(
            "{}: `comptime.for` has {} operands but {} results; it should be the same",
            cx.prefix,
            operands.len(),
            results.len()
        ));
        return;
    }
    for (index, (operand, result)) in operands.iter().zip(results).enumerate() {
        if let (Some(found), Some(expected)) = (
            cx.function.reg_types.get(&operand.0),
            cx.function.reg_types.get(&result.0),
        ) && !types_compatible(found, expected)
        {
            errors.push(format!(
                "{}: `comptime.for` operand #{index} has type {found} but corresponding result has type {expected}",
                cx.prefix
            ));
            return;
        }
    }
    let target = YieldTarget::Arguments;
    let mnemonic = "comptime.for";
    yielded_values(
        cx.prefix,
        cx.function,
        mnemonic,
        operands,
        args,
        target,
        errors,
    );
}

/// The arguments and results of the loop a `break`/`continue` with `label`
/// names: the nearest enclosing loop when unlabelled, else the nearest
/// carrying the label; `None` when no enclosing loop matches.
fn enclosing_loop(context: &RegionContext, label: Option<LoopLabel>) -> Option<(&[Reg], &[Reg])> {
    context.regions.iter().rev().find_map(|kind| match kind {
        RegionKind::LoopBody {
            label: found,
            args,
            results,
        } if label.is_none() || *found == label => Some((args.as_slice(), results.as_slice())),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::{MirProgram, SpanTable};
    use mojito_checked::checked::ComptimeSequence;
    use mojito_types::param_expr::{ParamContext, ParamId, ParamRef};
    use mojito_types::types::GenericConstraint;

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
            orelse: one(Vec::new(), MirTerm::ComptimeBreak(Vec::new())),
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
            operands: Vec::new(),
            args: Vec::new(),
            results: Vec::new(),
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
            else_args: Vec::new(),
            elifs: vec![crate::mir::MirElif {
                cond_args: Vec::new(),
                then_args: Vec::new(),
                cond: one(
                    Vec::new(),
                    MirTerm::ElifCond {
                        cond: Reg(0),
                        values: Vec::new(),
                    },
                ),
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
            operands: Vec::new(),
            args: Vec::new(),
            results: Vec::new(),
            body: one(Vec::new(), MirTerm::Yield(Vec::new())),
        }]);
        assert_finding(&findings(&misplaced), "`yield` outside the region it ends");
        let elif = function_with(vec![MirInstr::If {
            results: Vec::new(),
            cond: Reg(0),
            then: one(
                Vec::new(),
                MirTerm::ElifCond {
                    cond: Reg(0),
                    values: Vec::new(),
                },
            ),
            else_args: Vec::new(),
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
            else_args: Vec::new(),
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
            operands: Vec::new(),
            args: Vec::new(),
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
                    else_args: Vec::new(),
                    elifs: Vec::new(),
                    orelse: one(Vec::new(), MirTerm::Yield(Vec::new())),
                }],
                MirTerm::Continue {
                    label: None,
                    values: Vec::new(),
                },
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
            operands: Vec::new(),
            args: Vec::new(),
            results: vec![Reg(1)],
            body: one(
                vec![valued_loop(None, Vec::new(), outer, Vec::new())],
                MirTerm::Continue {
                    label: None,
                    values: Vec::new(),
                },
            ),
        }]);
        let found = findings(&nested);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_finding(&found, "`break` yields 0 values for 1 results");
    }

    /// A loop labelled `label` entered with `operands` as `args`, whose
    /// body continues `inner` with `values`.
    fn carrying_loop(
        label: Option<LoopLabel>,
        (operands, args): (Vec<Reg>, Vec<Reg>),
        inner: Option<LoopLabel>,
        values: Vec<Reg>,
    ) -> MirInstr {
        MirInstr::Loop {
            label,
            operands,
            args,
            results: Vec::new(),
            body: one(
                Vec::new(),
                MirTerm::Continue {
                    label: inner,
                    values,
                },
            ),
        }
    }

    #[test]
    fn a_continue_with_values_verifies() {
        let carried = (vec![Reg(2)], vec![Reg(1)]);
        let function = function_with(vec![carrying_loop(None, carried, None, vec![Reg(1)])]);
        assert_eq!(findings(&function), Vec::<String>::new());
    }

    #[test]
    fn a_continue_carrying_the_wrong_values_is_a_finding() {
        let carried = || (vec![Reg(2)], vec![Reg(1)]);
        let arity = function_with(vec![carrying_loop(None, carried(), None, Vec::new())]);
        assert_finding(
            &findings(&arity),
            "`continue` yields 0 values for 1 arguments",
        );
        let typed = function_with(vec![carrying_loop(None, carried(), None, vec![Reg(0)])]);
        assert_finding(
            &findings(&typed),
            "`continue` yields Bool for an argument of type Int",
        );
        // A labelled continue is checked against the loop it names, not
        // the nearest one.
        let outer = Some(LoopLabel(7));
        let nested = function_with(vec![MirInstr::Loop {
            label: outer,
            operands: vec![Reg(2)],
            args: vec![Reg(1)],
            results: Vec::new(),
            body: one(
                vec![carrying_loop(
                    None,
                    (Vec::new(), Vec::new()),
                    outer,
                    Vec::new(),
                )],
                MirTerm::Unreachable,
            ),
        }]);
        let found = findings(&nested);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_finding(&found, "`continue` yields 0 values for 1 arguments");
    }

    #[test]
    fn a_loop_whose_operands_do_not_match_its_arguments_is_a_finding() {
        let arity = function_with(vec![carrying_loop(
            None,
            (Vec::new(), vec![Reg(1)]),
            None,
            vec![Reg(1)],
        )]);
        assert_finding(&findings(&arity), "`loop` yields 0 values for 1 arguments");
        let typed = function_with(vec![carrying_loop(
            None,
            (vec![Reg(0)], vec![Reg(1)]),
            None,
            vec![Reg(1)],
        )]);
        assert_finding(
            &findings(&typed),
            "`loop` yields Bool for an argument of type Int",
        );
    }

    /// A `match` of two cases over a `Bool` subject: the first tests `r0`
    /// and advances on failure, the second always completes; a nested
    /// `if` in a case may leave it through either terminator.
    fn two_case_match() -> MirInstr {
        let test = MirInstr::If {
            results: Vec::new(),
            cond: Reg(0),
            then: one(Vec::new(), MirTerm::Yield(Vec::new())),
            else_args: Vec::new(),
            elifs: Vec::new(),
            orelse: one(Vec::new(), MirTerm::MatchNext(Vec::new())),
        };
        MirInstr::Match {
            results: Vec::new(),
            else_args: Vec::new(),
            cases: vec![
                crate::mir::MirCase {
                    args: Vec::new(),
                    body: one(vec![test], MirTerm::MatchComplete(Vec::new())),
                },
                crate::mir::MirCase {
                    args: Vec::new(),
                    body: one(Vec::new(), MirTerm::MatchComplete(Vec::new())),
                },
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
            results: Vec::new(),
            else_args: Vec::new(),
            cases: vec![crate::mir::MirCase {
                args: Vec::new(),
                body: one(
                    vec![MirInstr::Loop {
                        label: None,
                        operands: Vec::new(),
                        args: Vec::new(),
                        results: Vec::new(),
                        body: one(
                            vec![MirInstr::If {
                                results: Vec::new(),
                                cond: Reg(0),
                                then: one(Vec::new(), MirTerm::MatchComplete(Vec::new())),
                                else_args: Vec::new(),
                                elifs: Vec::new(),
                                orelse: one(
                                    Vec::new(),
                                    MirTerm::Break {
                                        label: None,
                                        values: Vec::new(),
                                    },
                                ),
                            }],
                            MirTerm::Continue {
                                label: None,
                                values: Vec::new(),
                            },
                        ),
                    }],
                    MirTerm::MatchNext(Vec::new()),
                ),
            }],
            orelse: one(Vec::new(), MirTerm::Yield(Vec::new())),
        }]);
        assert_eq!(findings(&nested), Vec::<String>::new());
    }

    #[test]
    fn a_match_terminator_outside_a_case_is_a_finding() {
        let next = function_with(vec![MirInstr::If {
            results: Vec::new(),
            cond: Reg(0),
            then: one(Vec::new(), MirTerm::MatchNext(Vec::new())),
            else_args: Vec::new(),
            elifs: Vec::new(),
            orelse: one(Vec::new(), MirTerm::MatchComplete(Vec::new())),
        }]);
        let found = findings(&next);
        assert_finding(&found, "`match.next` is not inside a match case");
        assert_finding(&found, "`match.complete` is not inside a match case");
        // The else region is an arm, not a case.
        let orelse = function_with(vec![MirInstr::Match {
            results: Vec::new(),
            else_args: Vec::new(),
            cases: vec![crate::mir::MirCase {
                args: Vec::new(),
                body: one(Vec::new(), MirTerm::MatchComplete(Vec::new())),
            }],
            orelse: one(Vec::new(), MirTerm::MatchNext(Vec::new())),
        }]);
        assert_finding(
            &findings(&orelse),
            "`match.next` is not inside a match case",
        );
        let empty = function_with(vec![MirInstr::Match {
            results: Vec::new(),
            else_args: Vec::new(),
            cases: Vec::new(),
            orelse: one(Vec::new(), MirTerm::Yield(Vec::new())),
        }]);
        assert_finding(&findings(&empty), "match holds no case");
    }

    /// [`function_with`] over `extra` more registers of the given types,
    /// numbered from 3.
    fn function_over(instrs: Vec<MirInstr>, extra: &[Ty]) -> MirFunction {
        let mut function = function_with(instrs);
        for (at, ty) in extra.iter().enumerate() {
            function.reg_types.insert(3 + at as u32, ty.clone());
        }
        function.n_regs = 3 + extra.len() as u32;
        function
    }

    /// An `if` on `%r0` with one elif per `(cond_args, values, then_args)`
    /// whose condition yields `%r0` and `values`, every arm yielding
    /// nothing.
    fn elif_chain(elifs: Vec<(Vec<Reg>, Vec<Reg>, Vec<Reg>)>, else_args: Vec<Reg>) -> MirInstr {
        let arm = || one(Vec::new(), MirTerm::Yield(Vec::new()));
        MirInstr::If {
            results: Vec::new(),
            cond: Reg(0),
            then: arm(),
            elifs: elifs
                .into_iter()
                .map(|(cond_args, values, then_args)| crate::mir::MirElif {
                    cond_args,
                    cond: one(
                        Vec::new(),
                        MirTerm::ElifCond {
                            cond: Reg(0),
                            values,
                        },
                    ),
                    then_args,
                    then: arm(),
                })
                .collect(),
            else_args,
            orelse: arm(),
        }
    }

    #[test]
    fn an_elif_carrying_values_verifies() {
        let chain = elif_chain(
            vec![
                (Vec::new(), vec![Reg(1)], vec![Reg(3)]),
                (vec![Reg(4)], vec![Reg(4)], vec![Reg(5)]),
            ],
            vec![Reg(6)],
        );
        let function = function_over(vec![chain], &[Ty::int(), Ty::int(), Ty::int(), Ty::int()]);
        assert_eq!(findings(&function), Vec::<String>::new());
    }

    #[test]
    fn an_elif_carrying_the_wrong_values_is_a_finding() {
        let arity = elif_chain(vec![(Vec::new(), Vec::new(), vec![Reg(3)])], Vec::new());
        let arity = function_over(vec![arity], &[Ty::int()]);
        assert_finding(
            &findings(&arity),
            "`if.elifcond.yield` yields 0 values for 1 arguments",
        );
        // The arm's arguments agree; the else region's do not.
        let typed = elif_chain(vec![(Vec::new(), vec![Reg(1)], vec![Reg(3)])], vec![Reg(4)]);
        let typed = function_over(vec![typed], &[Ty::int(), Ty::bool()]);
        let found = findings(&typed);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_finding(
            &found,
            "`if.elifcond.yield` yields Int for an argument of type Bool",
        );
    }

    #[test]
    fn an_if_entering_a_region_with_arguments_is_a_finding() {
        let first = elif_chain(vec![(vec![Reg(3)], Vec::new(), Vec::new())], Vec::new());
        let first = function_over(vec![first], &[Ty::int()]);
        assert_finding(&findings(&first), "`if` yields 0 values for 1 arguments");
        let orelse = function_over(vec![elif_chain(Vec::new(), vec![Reg(3)])], &[Ty::int()]);
        assert_finding(&findings(&orelse), "`if` yields 0 values for 1 arguments");
    }

    /// A `match` whose first case leaves through `first`'s `match.next`
    /// values, from an `if` arm (its guard), and whose second case takes
    /// `args` and leaves with `second`'s.
    fn carrying_match(first: Vec<Reg>, args: Vec<Reg>, second: Vec<Reg>) -> MirInstr {
        let guard = MirInstr::If {
            results: Vec::new(),
            cond: Reg(0),
            then: one(Vec::new(), MirTerm::Yield(Vec::new())),
            elifs: Vec::new(),
            else_args: Vec::new(),
            orelse: one(Vec::new(), MirTerm::MatchNext(first.clone())),
        };
        MirInstr::Match {
            results: Vec::new(),
            cases: vec![
                crate::mir::MirCase {
                    args: Vec::new(),
                    body: one(vec![guard], MirTerm::MatchNext(first)),
                },
                crate::mir::MirCase {
                    args,
                    body: one(Vec::new(), MirTerm::MatchNext(second)),
                },
            ],
            else_args: vec![Reg(4)],
            orelse: one(Vec::new(), MirTerm::Yield(Vec::new())),
        }
    }

    #[test]
    fn a_match_next_carrying_values_verifies() {
        let carried = carrying_match(vec![Reg(1)], vec![Reg(3)], vec![Reg(3)]);
        let function = function_over(vec![carried], &[Ty::int(), Ty::int()]);
        assert_eq!(findings(&function), Vec::<String>::new());
    }

    #[test]
    fn a_match_next_carrying_the_wrong_values_is_a_finding() {
        let arity = carrying_match(Vec::new(), vec![Reg(3)], vec![Reg(3)]);
        let arity = function_over(vec![arity], &[Ty::int(), Ty::int()]);
        assert_finding(
            &findings(&arity),
            "`match.next` yields 0 values for 1 arguments",
        );
        let typed = carrying_match(vec![Reg(1)], vec![Reg(3)], vec![Reg(0)]);
        let typed = function_over(vec![typed], &[Ty::int(), Ty::int()]);
        let found = findings(&typed);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_finding(
            &found,
            "`match.next` yields Bool for an argument of type Int",
        );
    }

    #[test]
    fn a_match_whose_first_case_takes_arguments_is_a_finding() {
        let function = function_over(
            vec![MirInstr::Match {
                results: Vec::new(),
                cases: vec![crate::mir::MirCase {
                    args: vec![Reg(3)],
                    body: one(Vec::new(), MirTerm::MatchComplete(Vec::new())),
                }],
                else_args: Vec::new(),
                orelse: one(Vec::new(), MirTerm::Yield(Vec::new())),
            }],
            &[Ty::int()],
        );
        assert_finding(
            &findings(&function),
            "`match` yields 0 values for 1 arguments",
        );
    }

    #[test]
    fn a_match_next_in_an_else_names_the_enclosing_case() {
        let outer = |args: Vec<Reg>| {
            let inner = MirInstr::Match {
                results: Vec::new(),
                cases: vec![crate::mir::MirCase {
                    args: Vec::new(),
                    body: one(Vec::new(), MirTerm::MatchComplete(Vec::new())),
                }],
                else_args: Vec::new(),
                orelse: one(Vec::new(), MirTerm::MatchNext(vec![Reg(1)])),
            };
            MirInstr::Match {
                results: Vec::new(),
                cases: vec![
                    crate::mir::MirCase {
                        args: Vec::new(),
                        body: one(vec![inner], MirTerm::MatchComplete(Vec::new())),
                    },
                    crate::mir::MirCase {
                        args,
                        body: one(Vec::new(), MirTerm::MatchComplete(Vec::new())),
                    },
                ],
                else_args: Vec::new(),
                orelse: one(Vec::new(), MirTerm::Yield(Vec::new())),
            }
        };
        let carried = function_over(vec![outer(vec![Reg(3)])], &[Ty::int()]);
        assert_eq!(findings(&carried), Vec::<String>::new());
        let dropped = function_with(vec![outer(Vec::new())]);
        assert_finding(
            &findings(&dropped),
            "`match.next` yields 1 values for 0 arguments",
        );
    }

    /// A `match` defining `results` whose one case completes with `first`
    /// and whose else region yields `orelse`.
    fn valued_match(results: Vec<Reg>, first: Vec<Reg>, orelse: Vec<Reg>) -> MirInstr {
        MirInstr::Match {
            results,
            cases: vec![crate::mir::MirCase {
                args: Vec::new(),
                body: one(Vec::new(), MirTerm::MatchComplete(first)),
            }],
            else_args: Vec::new(),
            orelse: one(Vec::new(), MirTerm::Yield(orelse)),
        }
    }

    #[test]
    fn a_valued_match_verifies() {
        let valued = valued_match(vec![Reg(3)], vec![Reg(1)], vec![Reg(2)]);
        let function = function_over(vec![valued], &[Ty::int()]);
        assert_eq!(findings(&function), Vec::<String>::new());
    }

    #[test]
    fn a_match_completing_with_the_wrong_values_is_a_finding() {
        let complete = valued_match(vec![Reg(3)], Vec::new(), vec![Reg(2)]);
        let complete = function_over(vec![complete], &[Ty::int()]);
        assert_finding(
            &findings(&complete),
            "`match.complete` yields 0 values for 1 results",
        );
        let orelse = valued_match(vec![Reg(3)], vec![Reg(1)], Vec::new());
        let orelse = function_over(vec![orelse], &[Ty::int()]);
        assert_finding(&findings(&orelse), "`yield` yields 0 values for 1 results");
        let typed = valued_match(vec![Reg(3)], vec![Reg(0)], vec![Reg(2)]);
        let typed = function_over(vec![typed], &[Ty::int()]);
        let found = findings(&typed);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_finding(
            &found,
            "`match.complete` yields Bool for a result of type Int",
        );
    }

    #[test]
    fn a_match_complete_in_an_else_names_the_enclosing_case() {
        let outer = |results: Vec<Reg>| {
            let inner = MirInstr::Match {
                results: Vec::new(),
                cases: vec![crate::mir::MirCase {
                    args: Vec::new(),
                    body: one(Vec::new(), MirTerm::MatchComplete(Vec::new())),
                }],
                else_args: Vec::new(),
                orelse: one(Vec::new(), MirTerm::MatchComplete(vec![Reg(1)])),
            };
            MirInstr::Match {
                results,
                cases: vec![crate::mir::MirCase {
                    args: Vec::new(),
                    body: one(vec![inner], MirTerm::MatchComplete(vec![Reg(2)])),
                }],
                else_args: Vec::new(),
                orelse: one(Vec::new(), MirTerm::Unreachable),
            }
        };
        let carried = function_over(vec![outer(vec![Reg(3)])], &[Ty::int()]);
        assert_eq!(findings(&carried), Vec::<String>::new());
        let dropped = function_with(vec![outer(Vec::new())]);
        assert_finding(
            &findings(&dropped),
            "`match.complete` yields 1 values for 0 results",
        );
    }

    /// A `comptime for` entering with `operands` as `args` and defining
    /// `results`, whose suite continues with `next` and whose stop arm
    /// breaks with `out`.
    fn valued_comptime_for(
        [operands, args, results]: [Vec<Reg>; 3],
        next: Vec<Reg>,
        out: Vec<Reg>,
    ) -> MirInstr {
        let source = sequence();
        let test = MirInstr::ComptimeIf {
            results: Vec::new(),
            conds: vec![source.stop_test()],
            arms: vec![one(Vec::new(), MirTerm::ComptimeContinue(next))],
            orelse: one(Vec::new(), MirTerm::ComptimeBreak(out)),
        };
        let mut op = bare_comptime_for(source, one(vec![test], MirTerm::Unreachable));
        if let MirInstr::ComptimeFor {
            operands: op_operands,
            args: op_args,
            results: op_results,
            ..
        } = &mut op
        {
            *op_operands = operands;
            *op_args = args;
            *op_results = results;
        }
        op
    }

    #[test]
    fn a_valued_comptime_for_verifies() {
        let values = [vec![Reg(1)], vec![Reg(3)], vec![Reg(4)]];
        let valued = valued_comptime_for(values, vec![Reg(3)], vec![Reg(3)]);
        let function = function_over(vec![valued], &[Ty::int(), Ty::int()]);
        assert_eq!(findings(&function), Vec::<String>::new());
    }

    #[test]
    fn a_comptime_for_exit_carrying_the_wrong_values_is_a_finding() {
        let values = || [vec![Reg(1)], vec![Reg(3)], vec![Reg(4)]];
        let arity = valued_comptime_for(values(), vec![Reg(3)], Vec::new());
        let arity = function_over(vec![arity], &[Ty::int(), Ty::int()]);
        assert_finding(
            &findings(&arity),
            "`comptime.for.break` yields 0 values for 1 results",
        );
        let typed = valued_comptime_for(values(), vec![Reg(0)], vec![Reg(3)]);
        let typed = function_over(vec![typed], &[Ty::int(), Ty::int()]);
        let found = findings(&typed);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_finding(
            &found,
            "`comptime.for.continue` yields Bool for an argument of type Int",
        );
    }

    #[test]
    fn a_comptime_for_whose_operands_and_results_differ_is_a_finding() {
        let values = [vec![Reg(1)], vec![Reg(3)], Vec::new()];
        let counted = valued_comptime_for(values, vec![Reg(3)], Vec::new());
        let counted = function_over(vec![counted], &[Ty::int()]);
        assert_finding(
            &findings(&counted),
            "`comptime.for` has 1 operands but 0 results; it should be the same",
        );
        let values = [vec![Reg(0)], vec![Reg(3)], vec![Reg(4)]];
        let typed = valued_comptime_for(values, vec![Reg(3)], vec![Reg(3)]);
        let typed = function_over(vec![typed], &[Ty::int(), Ty::int()]);
        assert_finding(
            &findings(&typed),
            "`comptime.for` operand #0 has type Bool but corresponding result has type Int",
        );
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
            else_args: Vec::new(),
            elifs: Vec::new(),
            orelse: one(Vec::new(), MirTerm::Yield(Vec::new())),
        }]);
        assert_finding(&findings(&unlabelled), "`break` has no enclosing loop");
        let labelled = function_with(vec![MirInstr::Loop {
            label: Some(LoopLabel(1)),
            operands: Vec::new(),
            args: Vec::new(),
            results: Vec::new(),
            body: one(
                Vec::new(),
                MirTerm::Continue {
                    label: Some(LoopLabel(2)),
                    values: Vec::new(),
                },
            ),
        }]);
        assert_finding(
            &findings(&labelled),
            "`continue` has no enclosing loop labelled loop2",
        );
        let named = function_with(vec![MirInstr::Loop {
            label: Some(LoopLabel(1)),
            operands: Vec::new(),
            args: Vec::new(),
            results: Vec::new(),
            body: one(
                vec![MirInstr::Loop {
                    label: None,
                    operands: Vec::new(),
                    args: Vec::new(),
                    results: Vec::new(),
                    body: one(
                        Vec::new(),
                        MirTerm::Break {
                            label: Some(LoopLabel(1)),
                            values: Vec::new(),
                        },
                    ),
                }],
                MirTerm::Continue {
                    label: None,
                    values: Vec::new(),
                },
            ),
        }]);
        assert_eq!(findings(&named), Vec::<String>::new());
    }

    #[test]
    fn a_comptime_break_needs_the_nearest_loop_to_be_a_comptime_for() {
        let through_loop = function_with(vec![comptime_for(one(
            vec![MirInstr::Loop {
                label: None,
                operands: Vec::new(),
                args: Vec::new(),
                results: Vec::new(),
                body: one(Vec::new(), MirTerm::ComptimeBreak(Vec::new())),
            }],
            MirTerm::ComptimeContinue(Vec::new()),
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
            MirTerm::ComptimeContinue(Vec::new()),
        ))]);
        assert_eq!(findings(&tested), Vec::<String>::new());
        let bare = function_with(vec![bare_comptime_for(
            sequence(),
            one(Vec::new(), MirTerm::ComptimeContinue(Vec::new())),
        )]);
        assert_finding(
            &findings(&bare),
            "comptime for body does not open with its has_next test",
        );
    }

    #[test]
    fn a_multi_block_region_is_rejected() {
        let function = function_with(vec![MirInstr::Loop {
            label: None,
            operands: Vec::new(),
            args: Vec::new(),
            results: Vec::new(),
            body: vec![
                MirBlock {
                    instrs: Vec::new(),
                    term: MirTerm::Break {
                        label: None,
                        values: Vec::new(),
                    },
                },
                MirBlock {
                    instrs: Vec::new(),
                    term: MirTerm::Return(None),
                },
            ],
        }]);
        assert_finding(&findings(&function), "region holds 2 blocks");
    }

    #[test]
    fn concrete_mir_carries_no_compile_time_control_flow() {
        let function = function_with(vec![MirInstr::ComptimeIf {
            results: Vec::new(),
            conds: vec![GenericConstraint::Bool(true)],
            arms: vec![one(Vec::new(), MirTerm::ComptimeYield(Vec::new()))],
            orelse: one(Vec::new(), MirTerm::ComptimeYield(Vec::new())),
        }]);
        let program = MirProgram {
            functions: vec![("f".into(), function)],
            declarations: MirDeclarations::default(),
            invariant_errors: Vec::new(),
        };
        let found = verify_concrete(&program);
        assert_finding(&found, "keeps a compile-time branch");
        assert_finding(
            &found,
            "keeps the region terminator `comptime.yield` in elaborated MIR",
        );
    }

    /// A raising `+` of the two `Int` registers into `%r1`, its flag `%r3`
    /// and error `%r4`, then `after`, in a raising body.
    fn raising_with(after: Vec<MirInstr>) -> MirFunction {
        let raises = crate::mir::Raises {
            ty: Ty::error(),
            flag: Reg(3),
            error: Reg(4),
        };
        let mut instrs = vec![MirInstr::BinOp {
            op: mojito_ast::ast::InfixOp::Add,
            dest: Reg(1),
            a: Reg(2),
            b: Reg(2),
            resolved: None,
            raises: Some(raises),
        }];
        instrs.extend(after);
        let mut function = function_with(instrs);
        function.n_regs = 5;
        function
            .reg_types
            .extend([(3, Ty::bool()), (4, Ty::error())]);
        function.raises = true;
        function.error_ty = Some(Ty::error());
        function
    }

    /// The error region on `%r3`, its `then` region raising `error`.
    fn error_region(error: Reg) -> MirInstr {
        MirInstr::If {
            results: Vec::new(),
            cond: Reg(3),
            then: one(Vec::new(), MirTerm::ErrorReturn { src: error }),
            elifs: Vec::new(),
            else_args: Vec::new(),
            orelse: one(Vec::new(), MirTerm::Yield(Vec::new())),
        }
    }

    #[test]
    fn a_raising_instruction_followed_by_its_error_region_verifies() {
        let found = findings(&raising_with(vec![error_region(Reg(4))]));
        assert!(
            !found.iter().any(|finding| finding.contains("error region")),
            "{found:?}"
        );
    }

    #[test]
    fn a_raising_instruction_without_its_error_region_is_a_finding() {
        let found = findings(&raising_with(Vec::new()));
        assert_finding(
            &found,
            "raising instruction has no error region on its flag %r3",
        );
    }

    #[test]
    fn an_error_region_raising_another_value_is_a_finding() {
        let found = findings(&raising_with(vec![error_region(Reg(2))]));
        assert_finding(&found, "error region on %r3 does not hand on %r4");
    }

    #[test]
    fn a_raise_naming_no_enclosing_try_is_a_finding() {
        let raising = |label| {
            function_with(vec![MirInstr::Try {
                label: crate::mir::TryLabel(0),
                error: None,
                results: Vec::new(),
                body: one(
                    Vec::new(),
                    MirTerm::TryRaise {
                        label: crate::mir::TryLabel(label),
                        values: Vec::new(),
                    },
                ),
                handler_args: Vec::new(),
                handler: Some(one(Vec::new(), MirTerm::TryYield(Vec::new()))),
                else_args: Vec::new(),
                orelse: None,
            }])
        };
        assert!(
            !findings(&raising(0))
                .iter()
                .any(|finding| finding.contains("names try"))
        );
        assert_finding(
            &findings(&raising(1)),
            "`try.raise` names try1, which no enclosing try body carries",
        );
    }

    /// A `try` labelled `try0` over [`function_with`]'s registers, plus
    /// `Int` registers `%r3` to `%r5` and the `Error` `%r6`: its body raises
    /// with `raised` from an `if` on `%r0`, then yields `yielded` to
    /// its `else`, which takes `%r3` and yields `%r3` to the result `%r4`;
    /// its handler takes `handler_args` and yields `%r5`.
    fn valued_try(raised: Vec<Reg>, yielded: Vec<Reg>, handler_args: Vec<Reg>) -> MirFunction {
        let label = crate::mir::TryLabel(0);
        let raise = MirInstr::If {
            results: Vec::new(),
            cond: Reg(0),
            then: one(
                Vec::new(),
                MirTerm::TryRaise {
                    label,
                    values: raised,
                },
            ),
            elifs: Vec::new(),
            else_args: Vec::new(),
            orelse: one(Vec::new(), MirTerm::Yield(Vec::new())),
        };
        let mut function = function_with(vec![MirInstr::Try {
            label,
            error: None,
            results: vec![Reg(4)],
            body: one(vec![raise], MirTerm::TryYield(yielded)),
            handler_args,
            handler: Some(one(Vec::new(), MirTerm::TryYield(vec![Reg(5)]))),
            else_args: vec![Reg(3)],
            orelse: Some(one(Vec::new(), MirTerm::TryYield(vec![Reg(3)]))),
        }]);
        function.n_regs = 7;
        function.reg_types.extend([
            (3, Ty::int()),
            (4, Ty::int()),
            (5, Ty::int()),
            (6, Ty::error()),
        ]);
        function
    }

    /// The findings of `function` about the values of a `try`'s edges.
    fn try_value_findings(function: &MirFunction) -> Vec<String> {
        findings(function)
            .into_iter()
            .filter(|finding| {
                ["yields", "`try.raise`", "`try` has"]
                    .iter()
                    .any(|topic| finding.contains(topic))
            })
            .collect()
    }

    #[test]
    fn a_valued_try_verifies() {
        let function = valued_try(vec![Reg(1)], vec![Reg(2)], vec![Reg(5)]);
        assert_eq!(try_value_findings(&function), Vec::<String>::new());
    }

    #[test]
    fn a_try_yield_carrying_the_wrong_values_is_a_finding() {
        let missing = valued_try(vec![Reg(1)], Vec::new(), vec![Reg(5)]);
        assert_finding(
            &try_value_findings(&missing),
            "`try.yield` yields 0 values for 1 arguments",
        );
        let mistyped = valued_try(vec![Reg(1)], vec![Reg(6)], vec![Reg(5)]);
        assert_finding(
            &try_value_findings(&mistyped),
            "`try.yield` yields Error for an argument of type Int",
        );
    }

    #[test]
    fn a_raise_carrying_the_wrong_values_is_a_finding() {
        let function = valued_try(vec![Reg(1), Reg(2)], vec![Reg(2)], vec![Reg(5)]);
        assert_finding(
            &try_value_findings(&function),
            "`try.raise` yields 2 values for 1 arguments",
        );
    }

    #[test]
    fn a_try_without_a_handler_taking_arguments_is_a_finding() {
        let mut function = valued_try(Vec::new(), vec![Reg(2)], vec![Reg(5)]);
        if let MirInstr::Try { handler, .. } = &mut function.blocks[0].instrs[0] {
            *handler = None;
        }
        assert_finding(
            &try_value_findings(&function),
            "`try` has 1 handler arguments but no handler",
        );
    }

    #[test]
    fn a_result_read_before_the_error_region_is_a_finding() {
        let read = MirInstr::DefVar {
            var: 0,
            src: Reg(1),
            binding_ty: None,
            in_place: false,
        };
        let found = findings(&raising_with(vec![read, error_region(Reg(4))]));
        assert_finding(
            &found,
            "%r1 is read before the error region decides it is defined",
        );
    }
}

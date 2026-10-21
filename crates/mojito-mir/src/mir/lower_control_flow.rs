//! The transitional `lower-control-flow` pass: structured region ops
//! (`if`, `loop`, `comptime if`, `comptime for`, `match`) to MIR's CFG, as
//! upstream's `LowerControlFlow` lowers `hlcf` to LLVM blocks.
//!
//! Each block list is rebuilt: the function's, and each `try` part's as its
//! own list. A list's own blocks keep their indices, so an escape target
//! already naming a function block stays valid; a region op's regions are
//! appended after them, and the instructions after the op move to a new join
//! block. An op's results travel through one fresh slot each, stored at each
//! `yield` or `break` and forwarded at the join, the shape lowering gives a
//! ternary. A loop left by one `break` in its own list is the exception: the
//! join is reached from that break alone, so each result is the value the
//! break carries, as a block argument with one predecessor is its operand
//! once upstream's `LowerControlFlow` has run. A break in a `try` part or a
//! `comptime for` body is not in that list: the first escapes, and the
//! elaborator copies the second per iteration.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

/// Lower every `if`, `loop`, `comptime if`, `comptime for`, and `match` of
/// `function` to blocks and CFG terminators. A body without them is
/// unchanged.
pub fn lower_control_flow(function: &mut MirFunction) {
    if !has_structured_control_flow(function) {
        return;
    }
    let blocks = std::mem::take(&mut function.blocks);
    let mut lowering = Lowering {
        function,
        out: Vec::new(),
        depth: 0,
        frames: Vec::new(),
        folded: HashMap::new(),
    };
    lowering.list(blocks);
    let Lowering {
        out: mut lowered,
        folded,
        ..
    } = lowering;
    replace_register_reads(&mut lowered, &resolve_replacements(&folded));
    function.blocks = lowered;
    function
        .reg_types
        .retain(|reg, _| !folded.contains_key(reg));
    function.spans.0.retain(|reg, _| !folded.contains_key(reg));
}

/// [`lower_control_flow`] over every function of `program`.
pub fn lower_control_flow_program(program: &mut MirProgram) {
    for (_, function) in &mut program.functions {
        lower_control_flow(function);
    }
}

/// Whether some block of `function`, at any depth, holds a region op other
/// than `try`.
pub fn has_structured_control_flow(function: &MirFunction) -> bool {
    blocks_hold_structure(&function.blocks)
}

/// The lowering of one function's block lists.
struct Lowering<'f> {
    function: &'f mut MirFunction,
    /// The block list being built.
    out: Vec<MirBlock>,
    /// How many `try` parts the list being built sits in: 0 is the
    /// function's own list.
    depth: usize,
    /// The loops enclosing the region being emitted, innermost last.
    frames: Vec<Frame>,
    /// Each result of a loop left by one `break` in its own list, and the
    /// value that break carries.
    folded: HashMap<u32, Reg>,
}

/// A loop a `break`/`continue` may name, or a `match` case a `match.next`/
/// `match.complete` leaves: where it continues (a loop's header, the next
/// case's entry), where it exits, the list its blocks are in, and the slots
/// a loop's `break` stores its values into.
struct Frame {
    kind: RegionFrame,
    header: MirBlockId,
    exit: MirBlockId,
    depth: usize,
    slots: Vec<VarId>,
}

/// How a region's own terminators end it.
#[derive(Clone)]
enum RegionEnd {
    /// A block list of its own: the function's, or a `try` part's.
    List,
    /// An `if` or `comptime if` arm: `yield` stores into the result slots
    /// and joins.
    Arm { join: MirBlockId, slots: Vec<VarId> },
    /// An elif condition: `if.elifcond.yield` branches to its arm or on.
    ElifCond { then: MirBlockId, next: MirBlockId },
    /// A loop body, or a `comptime for` body: its frame is on the stack.
    Body,
}

impl Lowering<'_> {
    /// Rebuild a block list of its own into `self.out`.
    fn list(&mut self, blocks: Vec<MirBlock>) {
        self.region(blocks, &RegionEnd::List);
    }

    /// Append `blocks` to the list being built, renumbering their local
    /// targets, and return the index of their entry.
    fn region(&mut self, blocks: Vec<MirBlock>, end: &RegionEnd) -> MirBlockId {
        let base = self.reserve(blocks.len());
        self.fill(blocks, base, end);
        base
    }

    /// Fill the blocks reserved at `base` from `blocks`.
    fn fill(&mut self, blocks: Vec<MirBlock>, base: MirBlockId, end: &RegionEnd) {
        for (index, block) in blocks.into_iter().enumerate() {
            self.block(block, base + index, base, end);
        }
    }

    fn reserve(&mut self, count: usize) -> MirBlockId {
        let base = self.out.len();
        self.out.extend((0..count).map(|_| MirBlock {
            instrs: Vec::new(),
            term: MirTerm::Unreachable,
        }));
        base
    }

    /// Fill block `at` from `block`, splitting it at each region op: the
    /// instructions after the op continue in the op's join block.
    fn block(&mut self, block: MirBlock, at: MirBlockId, base: usize, end: &RegionEnd) {
        let mut current = at;
        let mut instrs = Vec::new();
        for instruction in block.instrs {
            match instruction {
                MirInstr::If { .. }
                | MirInstr::Loop { .. }
                | MirInstr::ComptimeIf { .. }
                | MirInstr::ComptimeFor { .. }
                | MirInstr::Match { .. } => {
                    let join = self.reserve(1);
                    let (entry, prefix) = self.op(instruction, join);
                    self.out[current] = MirBlock {
                        instrs: std::mem::take(&mut instrs),
                        term: entry,
                    };
                    current = join;
                    instrs = prefix;
                }
                MirInstr::Try { .. } => instrs.push(self.try_parts(instruction)),
                _ => instrs.push(instruction),
            }
        }
        let term = self.term(block.term, base, end, &mut instrs);
        self.out[current] = MirBlock { instrs, term };
    }

    /// Lower a region op whose join block is `join`: the terminator that
    /// enters it, and the instructions that open the join. Any other
    /// instruction is entered by a jump and opens the join itself.
    fn op(&mut self, instruction: MirInstr, join: MirBlockId) -> (MirTerm, Vec<MirInstr>) {
        match instruction {
            MirInstr::If {
                results,
                cond,
                then,
                elifs,
                orelse,
            } => self.lower_if(&results, cond, then, elifs, orelse, join),
            MirInstr::Loop {
                label,
                results,
                mut body,
            } => {
                let slots = self.loop_slots(label, &results, &mut body);
                let kind = RegionFrame::Loop(label);
                let entry = self.lower_loop(kind, body, join, None, slots.clone());
                (MirTerm::Jump(entry), forwards(&results, &slots))
            }
            MirInstr::ComptimeIf {
                results,
                conds,
                arms,
                orelse,
            } => self.lower_comptime_if(&results, conds, arms, orelse, join),
            MirInstr::ComptimeFor {
                binder,
                source,
                body,
                orelse: _,
            } => {
                let header = self.reserve(1);
                let kind = RegionFrame::ComptimeFor;
                let body = self.lower_loop(kind, body, join, Some(header), Vec::new());
                self.out[header].term = MirTerm::ComptimeFor {
                    binder,
                    source,
                    body,
                    exit: join,
                };
                (MirTerm::Jump(header), Vec::new())
            }
            MirInstr::Match { cases, orelse } => {
                let entry = self.lower_match(cases, orelse, join);
                (MirTerm::Jump(entry), Vec::new())
            }
            other => (MirTerm::Jump(join), vec![other]),
        }
    }

    /// Lower a `match`: each case region under a frame whose `header` is
    /// the next case's entry (the else region's after the last) and whose
    /// `exit` is the join, so `match.next` and `match.complete` resolve to
    /// jumps; the else region yields to the join. Returns the first case's
    /// entry.
    fn lower_match(
        &mut self,
        cases: Vec<Vec<MirBlock>>,
        orelse: Vec<MirBlock>,
        join: MirBlockId,
    ) -> MirBlockId {
        let arm = RegionEnd::Arm {
            join,
            slots: Vec::new(),
        };
        let mut next = self.region(orelse, &arm);
        for blocks in cases.into_iter().rev() {
            let entry = self.reserve(blocks.len());
            self.frames.push(Frame {
                kind: RegionFrame::MatchCase,
                header: next,
                exit: join,
                depth: self.depth,
                slots: Vec::new(),
            });
            self.fill(blocks, entry, &RegionEnd::Body);
            self.frames.pop();
            next = entry;
        }
        next
    }

    fn lower_if(
        &mut self,
        results: &[Reg],
        cond: Reg,
        then: Vec<MirBlock>,
        elifs: Vec<MirElif>,
        orelse: Vec<MirBlock>,
        join: MirBlockId,
    ) -> (MirTerm, Vec<MirInstr>) {
        let slots = self.result_slots(results);
        let arm = RegionEnd::Arm {
            join,
            slots: slots.clone(),
        };
        let then_b = self.region(then, &arm);
        let mut next_tests = Vec::new();
        let mut arms = Vec::new();
        for elif in elifs {
            let test = self.reserve(elif.cond.len());
            next_tests.push((test, elif.cond));
            arms.push(self.region(elif.then, &arm));
        }
        let else_b = self.region(orelse, &arm);
        let mut next = else_b;
        for ((test, blocks), then) in next_tests.into_iter().zip(arms).rev() {
            self.fill(blocks, test, &RegionEnd::ElifCond { then, next });
            next = test;
        }
        let entry = MirTerm::Branch {
            cond,
            then_b,
            else_b: next,
        };
        (entry, forwards(results, &slots))
    }

    /// Lower a loop body under its frame, which continues at `header`, or
    /// at the body's entry when there is none, and whose breaks store into
    /// `slots`; return that entry.
    fn lower_loop(
        &mut self,
        kind: RegionFrame,
        body: Vec<MirBlock>,
        join: MirBlockId,
        header: Option<MirBlockId>,
        slots: Vec<VarId>,
    ) -> MirBlockId {
        let entry = self.reserve(body.len());
        self.frames.push(Frame {
            kind,
            header: header.unwrap_or(entry),
            exit: join,
            depth: self.depth,
            slots,
        });
        self.fill(body, entry, &RegionEnd::Body);
        self.frames.pop();
        entry
    }

    fn lower_comptime_if(
        &mut self,
        results: &[Reg],
        conds: Vec<GenericConstraint>,
        arms: Vec<Vec<MirBlock>>,
        orelse: Vec<MirBlock>,
        join: MirBlockId,
    ) -> (MirTerm, Vec<MirInstr>) {
        let slots = self.result_slots(results);
        let arm = RegionEnd::Arm {
            join,
            slots: slots.clone(),
        };
        let entries: Vec<MirBlockId> = arms
            .into_iter()
            .map(|blocks| self.region(blocks, &arm))
            .collect();
        let mut next = self.region(orelse, &arm);
        let mut tests = conds.into_iter().zip(entries).rev().peekable();
        let mut entry = None;
        while let Some((cond, then_b)) = tests.next() {
            let branch = MirTerm::ComptimeBranch {
                cond: Box::new(cond),
                then_b,
                else_b: next,
            };
            if tests.peek().is_none() {
                entry = Some(branch);
            } else {
                next = self.reserve(1);
                self.out[next].term = branch;
            }
        }
        let entry = entry.unwrap_or(MirTerm::Jump(next));
        (entry, forwards(results, &slots))
    }

    /// The slots a loop's results travel through: none when one `break` in
    /// the loop's own list leaves it, whose values the results fold to.
    fn loop_slots(
        &mut self,
        label: Option<LoopLabel>,
        results: &[Reg],
        body: &mut [MirBlock],
    ) -> Vec<VarId> {
        if results.is_empty() {
            return Vec::new();
        }
        let mut breaks = Vec::new();
        loop_breaks_mut(body, label, &mut |values, site| {
            breaks.push((values.clone(), site));
        });
        match breaks.as_slice() {
            [(values, site)] if *site == BreakSite::default() => {
                self.folded.extend(
                    results
                        .iter()
                        .zip(values)
                        .map(|(result, value)| (result.0, *value)),
                );
                Vec::new()
            }
            _ => self.result_slots(results),
        }
    }

    /// One fresh slot per result, typed by its register.
    fn result_slots(&mut self, results: &[Reg]) -> Vec<VarId> {
        results
            .iter()
            .map(|result| {
                let slot = self.function.n_vars as VarId;
                self.function.n_vars += 1;
                self.function.var_names.push(format!("$yield{slot}"));
                if let Some(ty) = self.function.reg_types.get(&result.0) {
                    self.function.var_tys.insert(slot, ty.clone());
                }
                slot
            })
            .collect()
    }

    /// A `try` whose parts are each rebuilt as a list of their own, one
    /// `try` deeper.
    fn try_parts(&mut self, mut instruction: MirInstr) -> MirInstr {
        for region in instruction_regions_mut(&mut instruction) {
            let outer = std::mem::take(&mut self.out);
            self.depth += 1;
            self.list(std::mem::take(region));
            self.depth -= 1;
            *region = std::mem::replace(&mut self.out, outer);
        }
        instruction
    }

    /// A terminator of a region based at `base`: its local targets
    /// renumbered, and a region terminator lowered to the edge it takes.
    fn term(
        &self,
        mut term: MirTerm,
        base: usize,
        end: &RegionEnd,
        instrs: &mut Vec<MirInstr>,
    ) -> MirTerm {
        for target in terminator_targets_mut(&mut term) {
            *target += base;
        }
        match (term, end) {
            (
                MirTerm::Yield(values) | MirTerm::ComptimeYield(values),
                RegionEnd::Arm { join, slots },
            ) => {
                instrs.extend(stores(&values, slots));
                MirTerm::Jump(*join)
            }
            (MirTerm::ElifCond(cond), RegionEnd::ElifCond { then, next }) => MirTerm::Branch {
                cond,
                then_b: *then,
                else_b: *next,
            },
            (MirTerm::Unreachable, _) => MirTerm::Return(None),
            (term, _) => match region_exit(&term) {
                Some((target, resume)) => {
                    let values = match &term {
                        MirTerm::Break { values, .. } => values.as_slice(),
                        _ => &[],
                    };
                    self.leave(instrs, target, resume, values).unwrap_or(term)
                }
                None => term,
            },
        }
    }

    /// The edge a region terminator naming `target` takes to the innermost
    /// frame it resolves to, its header when it `resume`s, else its exit: a
    /// jump within its list, an escape to the function's list
    /// from a `try` part, and the unsupported boundary from a `try` part
    /// nested in another. A `break`'s `values` are stored into the frame's
    /// slots first. `None` when no frame encloses it, which the verifier
    /// rejects.
    fn leave(
        &self,
        instrs: &mut Vec<MirInstr>,
        target: RegionFrame,
        resume: bool,
        values: &[Reg],
    ) -> Option<MirTerm> {
        let frame = self
            .frames
            .iter()
            .rev()
            .find(|frame| target.names(frame.kind))?;
        instrs.extend(stores(values, &frame.slots));
        let to = if resume { frame.header } else { frame.exit };
        Some(if frame.depth == self.depth {
            MirTerm::Jump(to)
        } else if frame.depth == 0 {
            MirTerm::EscapeJump {
                target: to,
                cleanup: Vec::new(),
            }
        } else {
            instrs.push(MirInstr::Unsupported(
                "try with break/continue crossing the try boundary".into(),
            ));
            MirTerm::TryYield
        })
    }
}

/// The stores of the values a `yield` or `break` carries, each into its
/// result's slot.
fn stores(values: &[Reg], slots: &[VarId]) -> Vec<MirInstr> {
    values
        .iter()
        .zip(slots)
        .map(|(src, var)| MirInstr::DefVar {
            var: *var,
            src: *src,
            binding_ty: None,
            in_place: false,
        })
        .collect()
}

/// The join's reads of an op's results, each forwarded out of its slot.
fn forwards(results: &[Reg], slots: &[VarId]) -> Vec<MirInstr> {
    results
        .iter()
        .zip(slots)
        .map(|(dest, var)| MirInstr::UseVar {
            dest: *dest,
            var: *var,
            mode: UseMode::Forward,
            constructor: None,
        })
        .collect()
}

fn blocks_hold_structure(blocks: &[MirBlock]) -> bool {
    blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .any(|instruction| {
            matches!(
                instruction,
                MirInstr::If { .. }
                    | MirInstr::Loop { .. }
                    | MirInstr::ComptimeIf { .. }
                    | MirInstr::ComptimeFor { .. }
                    | MirInstr::Match { .. }
            ) || instruction_regions(instruction)
                .into_iter()
                .any(|region| blocks_hold_structure(region))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(instrs: Vec<MirInstr>, term: MirTerm) -> Vec<MirBlock> {
        vec![MirBlock { instrs, term }]
    }

    fn function_with(blocks: Vec<MirBlock>) -> MirFunction {
        MirFunction {
            blocks,
            n_regs: 3,
            ret_ty: Some(Ty::none()),
            reg_types: HashMap::from([(0, Ty::bool()), (1, Ty::int()), (2, Ty::int())]),
            ..MirFunction::default()
        }
    }

    fn lowered(mut function: MirFunction) -> MirFunction {
        let declarations = MirDeclarations::default();
        assert_eq!(
            verify::verify_function_in("f", &function, &declarations),
            Vec::<String>::new()
        );
        lower_control_flow(&mut function);
        assert!(!has_structured_control_flow(&function));
        assert_eq!(
            verify::verify_function_in("f", &function, &declarations),
            Vec::<String>::new()
        );
        function
    }

    #[test]
    fn an_if_with_a_result_lowers_through_a_slot() {
        let function = lowered(function_with(one(
            vec![MirInstr::If {
                results: vec![Reg(1)],
                cond: Reg(0),
                then: one(Vec::new(), MirTerm::Yield(vec![Reg(2)])),
                elifs: vec![MirElif {
                    cond: one(Vec::new(), MirTerm::ElifCond(Reg(0))),
                    then: one(Vec::new(), MirTerm::Yield(vec![Reg(2)])),
                }],
                orelse: one(Vec::new(), MirTerm::Unreachable),
            }],
            MirTerm::Return(None),
        )));
        assert_eq!(function.n_vars, 1);
        assert_eq!(function.var_names, ["$yield0"]);
        assert!(matches!(
            function.blocks[0].term,
            MirTerm::Branch { cond: Reg(0), .. }
        ));
        let join = &function.blocks[1];
        assert!(matches!(
            join.instrs.as_slice(),
            [MirInstr::UseVar {
                dest: Reg(1),
                var: 0,
                mode: UseMode::Forward,
                constructor: None,
            }]
        ));
        assert!(matches!(join.term, MirTerm::Return(None)));
    }

    #[test]
    fn a_break_from_a_try_part_escapes_to_the_function_list() {
        let body = one(
            vec![MirInstr::Try {
                body: one(
                    Vec::new(),
                    MirTerm::Break {
                        label: None,
                        values: Vec::new(),
                    },
                ),
                handler: None,
                orelse: None,
                finalbody: None,
                cleanup: Vec::new(),
            }],
            MirTerm::Continue { label: None },
        );
        let function = lowered(function_with(one(
            vec![MirInstr::Loop {
                label: None,
                results: Vec::new(),
                body,
            }],
            MirTerm::Return(None),
        )));
        // bb0 enters the loop body at bb2; bb1 is the join after it.
        assert!(matches!(function.blocks[0].term, MirTerm::Jump(2)));
        assert!(matches!(function.blocks[2].term, MirTerm::Jump(2)));
        let MirInstr::Try { body, .. } = &function.blocks[2].instrs[0] else {
            panic!("the try stays");
        };
        assert!(matches!(
            body[0].term,
            MirTerm::EscapeJump { target: 1, .. }
        ));
    }

    /// A `match` of two cases: the first tests `r0` in a nested `if` and
    /// advances on failure, the second always completes.
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

    /// The blocks a chain of bare jumps from `from` passes through, the
    /// last being the first that is not a bare jump.
    fn chain(function: &MirFunction, mut from: MirBlockId) -> Vec<MirBlockId> {
        let mut visited = vec![from];
        while let MirTerm::Jump(to) = function.blocks[from].term
            && function.blocks[from].instrs.is_empty()
        {
            from = to;
            visited.push(from);
        }
        visited
    }

    #[test]
    fn match_next_and_complete_go_to_distinct_targets() {
        let function = lowered(function_with(one(
            vec![two_case_match()],
            MirTerm::Return(None),
        )));
        // bb0 enters the first case; bb1 is the join after the match.
        let MirTerm::Jump(first) = function.blocks[0].term else {
            panic!("the match is entered by a jump");
        };
        assert!(matches!(function.blocks[1].term, MirTerm::Return(None)));
        // The first case's test branches: its then arm yields back into the
        // case, which completes to the join; its else arm advances to the
        // second case, which completes to the join as well.
        let MirTerm::Branch {
            cond: Reg(0),
            then_b,
            else_b,
        } = function.blocks[first].term
        else {
            panic!("the first case tests r0");
        };
        let completed = chain(&function, then_b);
        let advanced = chain(&function, else_b);
        assert_eq!(completed.last(), Some(&1), "the completed first case joins");
        assert_eq!(
            advanced.last(),
            Some(&1),
            "the second case completes to the join"
        );
        // The failure path passes through the second case's own entry, which
        // the completion path never visits.
        assert!(
            advanced[..advanced.len() - 1]
                .iter()
                .any(|block| !completed.contains(block)),
            "next-case and completion take distinct paths: {completed:?} {advanced:?}"
        );
        assert!(!has_structured_control_flow(&function));
    }

    #[test]
    fn a_break_in_a_case_leaves_the_enclosing_loop() {
        let case = one(
            vec![MirInstr::If {
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
                orelse: one(Vec::new(), MirTerm::MatchNext),
            }],
            MirTerm::MatchComplete,
        );
        let body = one(
            vec![MirInstr::Match {
                cases: vec![case],
                orelse: one(Vec::new(), MirTerm::Yield(Vec::new())),
            }],
            MirTerm::Continue { label: None },
        );
        let function = lowered(function_with(one(
            vec![MirInstr::Loop {
                label: None,
                results: Vec::new(),
                body,
            }],
            MirTerm::Return(None),
        )));
        // bb0 enters the loop body; bb1 is the join after the loop.
        let MirTerm::Jump(loop_entry) = function.blocks[0].term else {
            panic!("the loop is entered by a jump");
        };
        assert!(matches!(function.blocks[1].term, MirTerm::Return(None)));
        let MirTerm::Jump(case_entry) = function.blocks[loop_entry].term else {
            panic!("the match is entered by a jump");
        };
        let MirTerm::Branch { then_b, else_b, .. } = function.blocks[case_entry].term else {
            panic!("the case tests r0");
        };
        // `break` leaves the loop, not the match; `match.next` reaches the
        // match's else region, which continues the loop.
        assert_eq!(chain(&function, then_b).last(), Some(&1));
        assert_eq!(chain(&function, else_b).last(), Some(&loop_entry));
    }

    /// A loop with result `r1` whose one `break`, carrying `r2`, sits in
    /// the suite of a `comptime for` its body holds.
    fn loop_left_from_a_comptime_for() -> MirInstr {
        use mojito_checked::checked::ComptimeSequence;
        use mojito_types::ct::CtValue;
        use mojito_types::param_expr::{ParamContext, ParamId, ParamRef};
        let context = ParamContext::detached();
        let constant = |value| context.constant(value).expect("a constant");
        let source = ComptimeSequence {
            initial: constant(CtValue::Int(0)),
            has_next: constant(CtValue::Bool(false)),
            next_iter: constant(CtValue::Int(1)),
            next_value: constant(CtValue::Int(0)),
        };
        let test = MirInstr::ComptimeIf {
            results: Vec::new(),
            conds: vec![source.stop_test()],
            arms: vec![one(
                Vec::new(),
                MirTerm::Break {
                    label: None,
                    values: vec![Reg(2)],
                },
            )],
            orelse: one(Vec::new(), MirTerm::ComptimeBreak),
        };
        let comptime_for = MirInstr::ComptimeFor {
            binder: ParamRef {
                id: ParamId::new("$comptime_for@f:0..1", 0),
                name: "i".into(),
            },
            source,
            body: one(vec![test], MirTerm::Unreachable),
            orelse: one(Vec::new(), MirTerm::Unreachable),
        };
        MirInstr::Loop {
            label: None,
            results: vec![Reg(1)],
            body: one(vec![comptime_for], MirTerm::Continue { label: None }),
        }
    }

    #[test]
    fn a_break_in_a_comptime_for_keeps_its_result_slot() {
        let function = lowered(function_with(one(
            vec![loop_left_from_a_comptime_for()],
            MirTerm::Return(None),
        )));
        assert_eq!(function.var_names, ["$yield0"]);
        let stores = function
            .blocks
            .iter()
            .flat_map(|block| &block.instrs)
            .filter(|instruction| {
                matches!(
                    instruction,
                    MirInstr::DefVar {
                        var: 0,
                        src: Reg(2),
                        ..
                    }
                )
            })
            .count();
        assert_eq!(stores, 1);
        assert!(matches!(
            function.blocks[1].instrs.as_slice(),
            [MirInstr::UseVar {
                dest: Reg(1),
                var: 0,
                mode: UseMode::Forward,
                ..
            }]
        ));
    }

    #[test]
    fn a_lowered_body_is_unchanged() {
        let mut function = function_with(vec![
            MirBlock {
                instrs: Vec::new(),
                term: MirTerm::Branch {
                    cond: Reg(0),
                    then_b: 1,
                    else_b: 0,
                },
            },
            MirBlock {
                instrs: Vec::new(),
                term: MirTerm::Return(None),
            },
        ]);
        let before = format!("{:?}", function.blocks);
        lower_control_flow(&mut function);
        assert_eq!(format!("{:?}", function.blocks), before);
    }
}

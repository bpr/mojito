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
//! once upstream's `LowerControlFlow` has run. A break in a `try` part is
//! not in that list, since it escapes. A loop's arguments travel
//! the same way, through one fresh slot each: its operands are stored
//! before the entering jump and each `continue`'s values before its jump to
//! the header, and the body's entry forwards them, as the entry and each
//! `continue` branch to the body block with its arguments upstream. A loop
//! no `continue` resumes enters its body from the entry alone, so each
//! argument is its operand. An `if` and a `match` enter their first region
//! with no values, and every later region's arguments travel the same way:
//! an `if.elifcond.yield` stores its values into its arm's slots and the
//! next region's, and each `match.next` into the next case's (the else
//! region's after the last case), and the region's entry forwards them. A
//! region entered from one `if.elifcond.yield` or one `match.next` in the
//! case's own list folds each argument to the value that terminator
//! carries. A `match`'s results travel like an `if`'s, stored at each
//! `match.complete` and at the else region's `yield`, and fold when one of
//! those edges, in the case's own list, reaches the join. A `comptime
//! for`'s arguments travel like a loop's, stored before the jump to its
//! header and at each `comptime.for.continue`, and fold to its operands
//! when nothing continues it; its results travel like a loop's, stored at
//! each `comptime.for.break`, and fold to the values of the one break in
//! its own list. The elaborator unrolls every `comptime for` on its region,
//! so only a phase's lowering meets one.

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
    /// value that break carries; each argument of a loop or `comptime for`
    /// no `continue` resumes, and its operand; each argument of an `if` or
    /// `match` region entered from one terminator, and each result of a
    /// `match` whose join one edge reaches, and the value it carries.
    folded: HashMap<u32, Reg>,
}

/// A loop a `break`/`continue` may name, a `comptime for` a
/// `comptime.for.break`/`continue` leaves, or a `match` case a
/// `match.next`/`match.complete` leaves: where it continues (a loop's
/// header, the next case's entry), where it exits, the list its blocks are
/// in, and the slots its leaving and resuming terminators store their
/// values into.
struct Frame {
    kind: RegionFrame,
    header: MirBlockId,
    exit: MirBlockId,
    depth: usize,
    slots: Vec<VarId>,
    arg_slots: Vec<VarId>,
}

/// How a region's own terminators end it.
#[derive(Clone)]
enum RegionEnd {
    /// A block list of its own: the function's, or a `try` part's.
    List,
    /// An `if` or `comptime if` arm: `yield` stores into the result slots
    /// and joins.
    Arm { join: MirBlockId, slots: Vec<VarId> },
    /// An elif condition: `if.elifcond.yield` stores its values into the
    /// slots of its arm's arguments and the next region's, and branches to
    /// its arm or on.
    ElifCond {
        then: MirBlockId,
        next: MirBlockId,
        then_slots: Vec<VarId>,
        next_slots: Vec<VarId>,
    },
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
                    let (entry, prefix) = self.op(instruction, join, &mut instrs);
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
    /// enters it, after any instructions it appends to `instrs`, and the
    /// instructions that open the join. Any other instruction is entered by
    /// a jump and opens the join itself.
    fn op(
        &mut self,
        instruction: MirInstr,
        join: MirBlockId,
        instrs: &mut Vec<MirInstr>,
    ) -> (MirTerm, Vec<MirInstr>) {
        match instruction {
            MirInstr::If {
                results,
                cond,
                then,
                elifs,
                else_args,
                orelse,
            } => self.lower_if(&results, cond, then, elifs, (else_args, orelse), join),
            MirInstr::Loop {
                label,
                operands,
                args,
                results,
                mut body,
            } => {
                let slots = self.loop_slots(label, &results, &mut body);
                let arg_slots = self.argument_slots(label, &operands, &args, &mut body);
                instrs.extend(stores(&operands, &arg_slots));
                let kind = RegionFrame::Loop(label);
                let entry =
                    self.lower_loop(kind, body, join, None, slots.clone(), arg_slots.clone());
                let opening = &mut self.out[entry].instrs;
                opening.splice(0..0, forwards(&args, &arg_slots));
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
                operands,
                args,
                results,
                mut body,
                orelse: _,
            } => {
                let slots = self.comptime_result_slots(&results, &mut body);
                let arg_slots = self.comptime_argument_slots(&operands, &args, &mut body);
                instrs.extend(stores(&operands, &arg_slots));
                let header = self.reserve(1);
                let kind = RegionFrame::ComptimeFor;
                let entry = self.lower_loop(
                    kind,
                    body,
                    join,
                    Some(header),
                    slots.clone(),
                    arg_slots.clone(),
                );
                let opening = &mut self.out[entry].instrs;
                opening.splice(0..0, forwards(&args, &arg_slots));
                self.out[header].term = MirTerm::ComptimeFor {
                    binder,
                    source,
                    body: entry,
                    exit: join,
                };
                (MirTerm::Jump(header), forwards(&results, &slots))
            }
            MirInstr::Match {
                results,
                mut cases,
                else_args,
                orelse,
            } => {
                let slots = self.match_slots(&results, &mut cases, &orelse);
                let entry = self.lower_match(cases, (else_args, orelse), join, &slots);
                (MirTerm::Jump(entry), forwards(&results, &slots))
            }
            other => (MirTerm::Jump(join), vec![other]),
        }
    }

    /// Lower a `match`: each case region under a frame whose `header` is
    /// the next case's entry (the else region's after the last), whose
    /// `exit` is the join, whose argument slots are the next region's, and
    /// whose result slots are `slots`, so `match.next` and `match.complete`
    /// resolve to jumps; the else region yields to the join. Returns the
    /// first case's entry.
    fn lower_match(
        &mut self,
        mut cases: Vec<MirCase>,
        (else_args, orelse): (Vec<Reg>, Vec<MirBlock>),
        join: MirBlockId,
        slots: &[VarId],
    ) -> MirBlockId {
        let mut entering = Vec::new();
        for index in 0..cases.len() {
            let mut nexts = Vec::new();
            match_nexts_mut(&mut cases[index].body, &mut |values, site| {
                nexts.push((values.clone(), site));
            });
            let args = cases.get(index + 1).map_or(&else_args, |next| &next.args);
            let args = args.clone();
            entering.push(self.edge_slots(&args, &nexts, "arg"));
        }
        let arm = RegionEnd::Arm {
            join,
            slots: slots.to_vec(),
        };
        let mut next = self.region(orelse, &arm);
        let mut args = else_args;
        for (case, arg_slots) in cases.into_iter().zip(entering).rev() {
            self.out[next]
                .instrs
                .splice(0..0, forwards(&args, &arg_slots));
            let entry = self.reserve(case.body.len());
            self.frames.push(Frame {
                kind: RegionFrame::MatchCase,
                header: next,
                exit: join,
                depth: self.depth,
                slots: slots.to_vec(),
                arg_slots,
            });
            self.fill(case.body, entry, &RegionEnd::Body);
            self.frames.pop();
            next = entry;
            args = case.args;
        }
        next
    }

    fn lower_if(
        &mut self,
        results: &[Reg],
        cond: Reg,
        then: Vec<MirBlock>,
        elifs: Vec<MirElif>,
        (else_args, orelse): (Vec<Reg>, Vec<MirBlock>),
        join: MirBlockId,
    ) -> (MirTerm, Vec<MirInstr>) {
        let slots = self.value_slots(results, "yield");
        let arm = RegionEnd::Arm {
            join,
            slots: slots.clone(),
        };
        let then_b = self.region(then, &arm);
        let mut tests = Vec::new();
        for (index, elif) in elifs.iter().enumerate() {
            let yields: Vec<_> = elif
                .cond
                .iter()
                .filter_map(|block| match &block.term {
                    MirTerm::ElifCond { values, .. } => {
                        Some((values.clone(), BreakSite::default()))
                    }
                    _ => None,
                })
                .collect();
            let next_args = elifs
                .get(index + 1)
                .map_or(&else_args, |next| &next.cond_args);
            let then_slots = self.edge_slots(&elif.then_args, &yields, "arg");
            let next_slots = self.edge_slots(next_args, &yields, "arg");
            tests.push((then_slots, next_slots));
        }
        let mut arms = Vec::new();
        for (elif, (then_slots, _)) in elifs.into_iter().zip(&tests) {
            let test = self.reserve(elif.cond.len());
            let then = self.region(elif.then, &arm);
            self.out[then]
                .instrs
                .splice(0..0, forwards(&elif.then_args, then_slots));
            arms.push((test, elif.cond_args, elif.cond, then));
        }
        let mut next = self.region(orelse, &arm);
        let mut args = else_args;
        for ((test, cond_args, blocks, then), (then_slots, next_slots)) in
            arms.into_iter().zip(tests).rev()
        {
            self.out[next]
                .instrs
                .splice(0..0, forwards(&args, &next_slots));
            let end = RegionEnd::ElifCond {
                then,
                next,
                then_slots,
                next_slots,
            };
            self.fill(blocks, test, &end);
            next = test;
            args = cond_args;
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
    /// `slots` and continues into `arg_slots`; return that entry.
    fn lower_loop(
        &mut self,
        kind: RegionFrame,
        body: Vec<MirBlock>,
        join: MirBlockId,
        header: Option<MirBlockId>,
        slots: Vec<VarId>,
        arg_slots: Vec<VarId>,
    ) -> MirBlockId {
        let entry = self.reserve(body.len());
        self.frames.push(Frame {
            kind,
            header: header.unwrap_or(entry),
            exit: join,
            depth: self.depth,
            slots,
            arg_slots,
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
        let slots = self.value_slots(results, "yield");
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
        self.edge_slots(results, &breaks, "yield")
    }

    /// The slots a loop's arguments travel through: none when no
    /// `continue` resumes the loop, whose operands the arguments fold to.
    fn argument_slots(
        &mut self,
        label: Option<LoopLabel>,
        operands: &[Reg],
        args: &[Reg],
        body: &mut [MirBlock],
    ) -> Vec<VarId> {
        if args.is_empty() {
            return Vec::new();
        }
        let mut continued = false;
        loop_continues_mut(body, label, &mut |_, _| continued = true);
        self.carried_slots(continued, operands, args)
    }

    /// The slots loop-carried `args` travel through when `continued`, else
    /// none, each folded to its operand of `operands`.
    fn carried_slots(&mut self, continued: bool, operands: &[Reg], args: &[Reg]) -> Vec<VarId> {
        if continued {
            return self.value_slots(args, "arg");
        }
        self.folded.extend(
            args.iter()
                .zip(operands)
                .map(|(arg, operand)| (arg.0, *operand)),
        );
        Vec::new()
    }

    /// The slots a `match`'s results travel through: none when one edge
    /// reaches its join, a `match.complete` in a case's own list or the
    /// else region's `yield`, whose values the results fold to.
    fn match_slots(
        &mut self,
        results: &[Reg],
        cases: &mut [MirCase],
        orelse: &[MirBlock],
    ) -> Vec<VarId> {
        if results.is_empty() {
            return Vec::new();
        }
        let mut edges = Vec::new();
        for case in cases {
            match_completes_mut(&mut case.body, &mut |values, site| {
                edges.push((values.clone(), site));
            });
        }
        edges.extend(orelse.iter().filter_map(|block| match &block.term {
            MirTerm::Yield(values) => Some((values.clone(), BreakSite::default())),
            _ => None,
        }));
        self.edge_slots(results, &edges, "yield")
    }

    /// The slots a `comptime for`'s results travel through: none when one
    /// `comptime.for.break` in the body's own list leaves it, as
    /// [`Self::loop_slots`] folds a loop's.
    fn comptime_result_slots(&mut self, results: &[Reg], body: &mut [MirBlock]) -> Vec<VarId> {
        if results.is_empty() {
            return Vec::new();
        }
        let mut breaks = Vec::new();
        comptime_for_breaks_mut(body, &mut |values, site| {
            breaks.push((values.clone(), site));
        });
        self.edge_slots(results, &breaks, "yield")
    }

    /// The slots a `comptime for`'s arguments travel through: none when no
    /// `comptime.for.continue` resumes it, whose operands the arguments
    /// fold to, as [`Self::argument_slots`] folds a loop's.
    fn comptime_argument_slots(
        &mut self,
        operands: &[Reg],
        args: &[Reg],
        body: &mut [MirBlock],
    ) -> Vec<VarId> {
        if args.is_empty() {
            return Vec::new();
        }
        let mut continued = false;
        comptime_for_continues_mut(body, &mut |_, _| continued = true);
        self.carried_slots(continued, operands, args)
    }

    /// The slots the registers `defined` travel through, defined along
    /// `edges`, each carrying its values: none when one edge in the op's own
    /// list defines them, whose values they fold to; else one slot each,
    /// named after `prefix`.
    fn edge_slots(
        &mut self,
        defined: &[Reg],
        edges: &[(Vec<Reg>, BreakSite)],
        prefix: &str,
    ) -> Vec<VarId> {
        if defined.is_empty() {
            return Vec::new();
        }
        match edges {
            [(values, site)] if *site == BreakSite::default() => {
                self.folded.extend(
                    defined
                        .iter()
                        .zip(values)
                        .map(|(reg, value)| (reg.0, *value)),
                );
                Vec::new()
            }
            _ => self.value_slots(defined, prefix),
        }
    }

    /// One fresh slot per register of `regs`, typed by it and named
    /// `${prefix}{slot}`.
    fn value_slots(&mut self, regs: &[Reg], prefix: &str) -> Vec<VarId> {
        regs.iter()
            .map(|result| {
                let slot = self.function.n_vars as VarId;
                self.function.n_vars += 1;
                self.function.var_names.push(format!("${prefix}{slot}"));
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
            (
                MirTerm::ElifCond { cond, values },
                RegionEnd::ElifCond {
                    then,
                    next,
                    then_slots,
                    next_slots,
                },
            ) => {
                instrs.extend(stores(&values, then_slots));
                instrs.extend(stores(&values, next_slots));
                MirTerm::Branch {
                    cond,
                    then_b: *then,
                    else_b: *next,
                }
            }
            (MirTerm::Unreachable, _) => MirTerm::Return(None),
            (term, _) => match region_exit(&term) {
                Some((target, resume)) => {
                    let values = match &term {
                        MirTerm::Break { values, .. }
                        | MirTerm::Continue { values, .. }
                        | MirTerm::ComptimeBreak(values)
                        | MirTerm::ComptimeContinue(values)
                        | MirTerm::MatchNext(values)
                        | MirTerm::MatchComplete(values) => values.as_slice(),
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
    /// nested in another. A leaving terminator's `values` are stored into
    /// the frame's result slots first, a resuming one's (`continue`,
    /// `comptime.for.continue`, `match.next`) into its argument slots. `None`
    /// when no frame encloses it, which the verifier rejects.
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
        let slots = if resume {
            &frame.arg_slots
        } else {
            &frame.slots
        };
        instrs.extend(stores(values, slots));
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

/// The stores of the values a `yield`, `if.elifcond.yield`, or a region
/// exit carries, or of a loop's or `comptime for`'s operands, each into the
/// slot of what it defines.
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

/// The join's reads of an op's results, or a region's reads of its
/// arguments, each forwarded out of its slot.
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
                else_args: Vec::new(),
                elifs: vec![MirElif {
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
            MirTerm::Continue {
                label: None,
                values: Vec::new(),
            },
        );
        let function = lowered(function_with(one(
            vec![MirInstr::Loop {
                label: None,
                operands: Vec::new(),
                args: Vec::new(),
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
            else_args: Vec::new(),
            elifs: Vec::new(),
            orelse: one(Vec::new(), MirTerm::MatchNext(Vec::new())),
        };
        MirInstr::Match {
            results: Vec::new(),
            else_args: Vec::new(),
            cases: vec![
                MirCase {
                    args: Vec::new(),
                    body: one(vec![test], MirTerm::MatchComplete(Vec::new())),
                },
                MirCase {
                    args: Vec::new(),
                    body: one(Vec::new(), MirTerm::MatchComplete(Vec::new())),
                },
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
                else_args: Vec::new(),
                elifs: Vec::new(),
                orelse: one(Vec::new(), MirTerm::MatchNext(Vec::new())),
            }],
            MirTerm::MatchComplete(Vec::new()),
        );
        let body = one(
            vec![MirInstr::Match {
                results: Vec::new(),
                else_args: Vec::new(),
                cases: vec![MirCase {
                    args: Vec::new(),
                    body: case,
                }],
                orelse: one(Vec::new(), MirTerm::Yield(Vec::new())),
            }],
            MirTerm::Continue {
                label: None,
                values: Vec::new(),
            },
        );
        let function = lowered(function_with(one(
            vec![MirInstr::Loop {
                label: None,
                operands: Vec::new(),
                args: Vec::new(),
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
        // match's else region, which continues the loop: the bare jumps run
        // through the loop's entry, itself a bare jump into the match.
        assert_eq!(chain(&function, then_b).last(), Some(&1));
        assert!(chain(&function, else_b).contains(&loop_entry));
    }

    #[test]
    fn a_loop_with_arguments_lowers_through_slots() {
        let body = one(
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
                else_args: Vec::new(),
                elifs: Vec::new(),
                orelse: one(Vec::new(), MirTerm::Yield(Vec::new())),
            }],
            MirTerm::Continue {
                label: None,
                values: vec![Reg(1)],
            },
        );
        let function = lowered(function_with(one(
            vec![MirInstr::Loop {
                label: None,
                operands: vec![Reg(2)],
                args: vec![Reg(1)],
                results: Vec::new(),
                body,
            }],
            MirTerm::Return(None),
        )));
        assert_eq!(function.var_names, ["$arg0"]);
        let stores = |block: &MirBlock, from: Reg| {
            matches!(
                block.instrs.as_slice(),
                [MirInstr::DefVar { var: 0, src, .. }] if *src == from
            )
        };
        // The operand is stored before the entering jump, and the body's
        // entry forwards the argument out of its slot.
        assert!(stores(&function.blocks[0], Reg(2)));
        let MirTerm::Jump(entry) = function.blocks[0].term else {
            panic!("the loop is entered by a jump");
        };
        assert!(matches!(
            function.blocks[entry].instrs.first(),
            Some(MirInstr::UseVar {
                dest: Reg(1),
                var: 0,
                mode: UseMode::Forward,
                constructor: None,
            })
        ));
        // The `continue` stores its value before its jump to the entry.
        let continued = function.blocks.iter().any(|block| {
            stores(block, Reg(1)) && matches!(block.term, MirTerm::Jump(to) if to == entry)
        });
        assert!(continued, "{:?}", function.blocks);
    }

    #[test]
    fn a_loop_never_continued_folds_its_arguments() {
        let mut function = function_with(one(
            vec![MirInstr::Loop {
                label: None,
                operands: vec![Reg(2)],
                args: vec![Reg(1)],
                results: Vec::new(),
                body: one(
                    vec![MirInstr::DefVar {
                        var: 0,
                        src: Reg(1),
                        binding_ty: None,
                        in_place: false,
                    }],
                    MirTerm::Break {
                        label: None,
                        values: Vec::new(),
                    },
                ),
            }],
            MirTerm::Return(None),
        ));
        function.n_vars = 1;
        function.var_names = vec!["x".into()];
        function.var_tys = HashMap::from([(0, Ty::int())]);
        let function = lowered(function);
        assert_eq!(function.n_vars, 1, "no slot is made for the argument");
        let reads: Vec<Reg> = function
            .blocks
            .iter()
            .flat_map(|block| &block.instrs)
            .filter_map(|instruction| match instruction {
                MirInstr::DefVar { src, .. } => Some(*src),
                _ => None,
            })
            .collect();
        assert_eq!(reads, [Reg(2)]);
        assert!(!function.reg_types.contains_key(&1));
    }

    /// A `comptime for` taking `operands` into `args` and defining
    /// `results`, whose `has_next` test runs `suite` and leaves through
    /// `stop` when the sequence ends.
    fn comptime_for(
        suite: Vec<MirBlock>,
        stop: MirTerm,
        [operands, args, results]: [Vec<Reg>; 3],
    ) -> MirInstr {
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
            arms: vec![suite],
            orelse: one(Vec::new(), stop),
        };
        MirInstr::ComptimeFor {
            binder: ParamRef {
                id: ParamId::new("$comptime_for@f:0..1", 0),
                name: "i".into(),
            },
            source,
            operands,
            args,
            results,
            body: one(vec![test], MirTerm::Unreachable),
            orelse: one(Vec::new(), MirTerm::Unreachable),
        }
    }

    /// A loop with result `r1` whose one `break`, carrying `r2`, sits in
    /// the suite of a `comptime for` its body holds.
    fn loop_left_from_a_comptime_for() -> MirInstr {
        let suite = one(
            Vec::new(),
            MirTerm::Break {
                label: None,
                values: vec![Reg(2)],
            },
        );
        let stop = MirTerm::ComptimeBreak(Vec::new());
        let comptime_for = comptime_for(suite, stop, Default::default());
        MirInstr::Loop {
            label: None,
            operands: Vec::new(),
            args: Vec::new(),
            results: vec![Reg(1)],
            body: one(
                vec![comptime_for],
                MirTerm::Continue {
                    label: None,
                    values: Vec::new(),
                },
            ),
        }
    }

    #[test]
    fn a_break_in_a_comptime_for_folds_its_result() {
        let function = lowered(function_with(one(
            vec![loop_left_from_a_comptime_for()],
            MirTerm::Return(None),
        )));
        assert!(function.var_names.is_empty());
        assert!(!function.reg_types.contains_key(&1));
        assert!(
            function
                .blocks
                .iter()
                .flat_map(|block| &block.instrs)
                .all(|instruction| !matches!(instruction, MirInstr::DefVar { .. }))
        );
    }

    /// [`function_with`] over `bodies` with one `Int` slot `x` and the
    /// `Int` registers `r3` to `r{2 + extra}`.
    fn function_over(bodies: Vec<MirBlock>, extra: u32) -> MirFunction {
        let mut function = function_with(bodies);
        function.n_regs = 3 + extra;
        function
            .reg_types
            .extend((3..3 + extra).map(|reg| (reg, Ty::int())));
        function.n_vars = 1;
        function.var_names = vec!["x".into()];
        function.var_tys = HashMap::from([(0, Ty::int())]);
        function
    }

    /// A region storing `reg` into the slot `x`, ended by `term`.
    fn storing(reg: Reg, term: MirTerm) -> Vec<MirBlock> {
        let store = MirInstr::DefVar {
            var: 0,
            src: reg,
            binding_ty: None,
            in_place: false,
        };
        one(vec![store], term)
    }

    /// The registers every store into the slot `x` reads, in block order.
    fn stored(function: &MirFunction) -> Vec<Reg> {
        function
            .blocks
            .iter()
            .flat_map(|block| &block.instrs)
            .filter_map(|instruction| match instruction {
                MirInstr::DefVar { var: 0, src, .. } => Some(*src),
                _ => None,
            })
            .collect()
    }

    /// An `if` on `r0` whose one elif's condition region `cond` enters an
    /// arm taking `r3` and an else region taking `r4`, each storing its
    /// argument.
    fn carrying_elif(cond: Vec<MirBlock>) -> MirInstr {
        MirInstr::If {
            results: Vec::new(),
            cond: Reg(0),
            then: one(Vec::new(), MirTerm::Yield(Vec::new())),
            elifs: vec![MirElif {
                cond_args: Vec::new(),
                cond,
                then_args: vec![Reg(3)],
                then: storing(Reg(3), MirTerm::Yield(Vec::new())),
            }],
            else_args: vec![Reg(4)],
            orelse: storing(Reg(4), MirTerm::Yield(Vec::new())),
        }
    }

    #[test]
    fn an_elif_carrying_values_folds_its_arguments() {
        let cond = one(
            Vec::new(),
            MirTerm::ElifCond {
                cond: Reg(0),
                values: vec![Reg(2)],
            },
        );
        let body = one(vec![carrying_elif(cond)], MirTerm::Return(None));
        let function = lowered(function_over(body, 2));
        assert_eq!(function.n_vars, 1, "no slot is made for an argument");
        assert_eq!(stored(&function), [Reg(2), Reg(2)]);
        assert!(!function.reg_types.contains_key(&3));
        assert!(!function.reg_types.contains_key(&4));
    }

    #[test]
    fn an_elif_condition_of_two_blocks_carries_values_through_slots() {
        let ends = |value| MirBlock {
            instrs: Vec::new(),
            term: MirTerm::ElifCond {
                cond: Reg(0),
                values: vec![value],
            },
        };
        let cond = vec![
            MirBlock {
                instrs: Vec::new(),
                term: MirTerm::Branch {
                    cond: Reg(0),
                    then_b: 1,
                    else_b: 2,
                },
            },
            ends(Reg(1)),
            ends(Reg(2)),
        ];
        let body = one(vec![carrying_elif(cond)], MirTerm::Return(None));
        let function = lowered(function_over(body, 2));
        assert_eq!(function.var_names, ["x", "$arg1", "$arg2"]);
        // Each `if.elifcond.yield` stores its value into both regions'
        // slots before it branches.
        for value in [Reg(1), Reg(2)] {
            let carries = function.blocks.iter().any(|block| {
                matches!(
                    block.instrs.as_slice(),
                    [
                        MirInstr::DefVar { var: 1, src: a, .. },
                        MirInstr::DefVar { var: 2, src: b, .. },
                    ] if *a == value && *b == value
                ) && matches!(block.term, MirTerm::Branch { .. })
            });
            assert!(carries, "{:?}", function.blocks);
        }
        // The arm and the else region forward their argument first.
        for (dest, var) in [(Reg(3), 1), (Reg(4), 2)] {
            let forwarded = function.blocks.iter().any(|block| {
                matches!(
                    block.instrs.first(),
                    Some(MirInstr::UseVar { dest: d, var: v, mode: UseMode::Forward, .. })
                        if *d == dest && *v == var
                )
            });
            assert!(forwarded, "{:?}", function.blocks);
        }
    }

    /// A `match` whose first case leaves through `first` and whose second
    /// takes `r3`, stores it, and leaves with it to an else region taking
    /// `r4`, which stores that.
    fn carrying_match(first: Vec<MirBlock>) -> MirInstr {
        MirInstr::Match {
            results: Vec::new(),
            cases: vec![
                MirCase {
                    args: Vec::new(),
                    body: first,
                },
                MirCase {
                    args: vec![Reg(3)],
                    body: storing(Reg(3), MirTerm::MatchNext(vec![Reg(3)])),
                },
            ],
            else_args: vec![Reg(4)],
            orelse: storing(Reg(4), MirTerm::Yield(Vec::new())),
        }
    }

    #[test]
    fn a_case_entered_by_two_match_nexts_takes_its_arguments_through_slots() {
        let guard = MirInstr::If {
            results: Vec::new(),
            cond: Reg(0),
            then: one(Vec::new(), MirTerm::Yield(Vec::new())),
            elifs: Vec::new(),
            else_args: Vec::new(),
            orelse: one(Vec::new(), MirTerm::MatchNext(vec![Reg(1)])),
        };
        let first = one(vec![guard], MirTerm::MatchNext(vec![Reg(2)]));
        let body = one(vec![carrying_match(first)], MirTerm::Return(None));
        let function = lowered(function_over(body, 2));
        // The second case's argument takes a slot; the else region, entered
        // from the second case's one `match.next`, folds to `r3`.
        assert_eq!(function.var_names, ["x", "$arg1"]);
        let stores: Vec<Reg> = function
            .blocks
            .iter()
            .flat_map(|block| &block.instrs)
            .filter_map(|instruction| match instruction {
                MirInstr::DefVar { var: 1, src, .. } => Some(*src),
                _ => None,
            })
            .collect();
        assert_eq!(stores.len(), 2);
        assert!(stores.contains(&Reg(1)) && stores.contains(&Reg(2)));
        let entry = function.blocks.iter().find(|block| {
            matches!(
                block.instrs.first(),
                Some(MirInstr::UseVar {
                    dest: Reg(3),
                    var: 1,
                    mode: UseMode::Forward,
                    ..
                })
            )
        });
        assert!(entry.is_some(), "{:?}", function.blocks);
        assert_eq!(stored(&function), [Reg(3), Reg(3)]);
        assert!(!function.reg_types.contains_key(&4));
    }

    #[test]
    fn a_case_entered_by_one_match_next_folds_its_arguments() {
        let first = one(Vec::new(), MirTerm::MatchNext(vec![Reg(2)]));
        let body = one(vec![carrying_match(first)], MirTerm::Return(None));
        let function = lowered(function_over(body, 2));
        assert_eq!(function.n_vars, 1, "no slot is made for an argument");
        assert_eq!(stored(&function), [Reg(2), Reg(2)]);
        assert!(!function.reg_types.contains_key(&3));
        assert!(!function.reg_types.contains_key(&4));
    }

    /// A `match` defining `r3` whose first case leaves through `first`
    /// and whose second completes with `r2`, its else region `orelse`;
    /// the slot `x` stores `r3` after it.
    fn valued_match(first: Vec<MirBlock>, orelse: Vec<MirBlock>) -> Vec<MirBlock> {
        let op = MirInstr::Match {
            results: vec![Reg(3)],
            cases: vec![
                MirCase {
                    args: Vec::new(),
                    body: first,
                },
                MirCase {
                    args: Vec::new(),
                    body: one(Vec::new(), MirTerm::MatchComplete(vec![Reg(2)])),
                },
            ],
            else_args: Vec::new(),
            orelse,
        };
        storing_after(op, Reg(3))
    }

    /// The block a bare `jump` goes to.
    fn jump_target(term: &MirTerm) -> Option<MirBlockId> {
        match term {
            MirTerm::Jump(to) => Some(*to),
            _ => None,
        }
    }

    /// `op`, then a store of `reg` into the slot `x`.
    fn storing_after(op: MirInstr, reg: Reg) -> Vec<MirBlock> {
        let store = MirInstr::DefVar {
            var: 0,
            src: reg,
            binding_ty: None,
            in_place: false,
        };
        one(vec![op, store], MirTerm::Return(None))
    }

    #[test]
    fn a_match_completed_twice_takes_its_results_through_a_slot() {
        let first = one(Vec::new(), MirTerm::MatchComplete(vec![Reg(1)]));
        let orelse = one(Vec::new(), MirTerm::Yield(vec![Reg(1)]));
        let function = lowered(function_over(valued_match(first, orelse), 1));
        assert_eq!(function.var_names, ["x", "$yield1"]);
        // Both completions and the else region's `yield` store their value
        // before their jump to the join, which forwards the result.
        let stores: Vec<(Reg, Option<MirBlockId>)> = function
            .blocks
            .iter()
            .filter_map(|block| match block.instrs.as_slice() {
                [MirInstr::DefVar { var: 1, src, .. }] => Some((*src, jump_target(&block.term))),
                _ => None,
            })
            .collect();
        assert_eq!(
            stores,
            [(Reg(1), Some(1)), (Reg(2), Some(1)), (Reg(1), Some(1)),],
            "{:?}",
            function.blocks
        );
        assert!(matches!(
            function.blocks[1].instrs.as_slice(),
            [
                MirInstr::UseVar {
                    dest: Reg(3),
                    var: 1,
                    mode: UseMode::Forward,
                    constructor: None,
                },
                MirInstr::DefVar {
                    var: 0,
                    src: Reg(3),
                    ..
                },
            ]
        ));
    }

    #[test]
    fn a_match_completed_once_folds_its_results() {
        let first = one(Vec::new(), MirTerm::MatchNext(Vec::new()));
        let orelse = one(Vec::new(), MirTerm::Unreachable);
        let function = lowered(function_over(valued_match(first, orelse), 1));
        assert_eq!(function.n_vars, 1, "no slot is made for the result");
        assert_eq!(stored(&function), [Reg(2)]);
        assert!(!function.reg_types.contains_key(&3));
    }

    /// The `comptime for` of [`comptime_for`] taking `r1` into `r3` and
    /// defining `r4`, whose suite ends in `next` and whose stop arm breaks
    /// with `r3`; the slot `x` stores `r4` after it.
    fn carrying_comptime_for(next: MirTerm) -> Vec<MirBlock> {
        let values = [vec![Reg(1)], vec![Reg(3)], vec![Reg(4)]];
        let stop = MirTerm::ComptimeBreak(vec![Reg(3)]);
        storing_after(comptime_for(one(Vec::new(), next), stop, values), Reg(4))
    }

    #[test]
    fn a_comptime_for_carries_its_arguments_through_slots() {
        let body = carrying_comptime_for(MirTerm::ComptimeContinue(vec![Reg(2)]));
        let function = lowered(function_over(body, 2));
        assert_eq!(function.var_names, ["x", "$arg1"]);
        let block_storing = |var: VarId, value: Reg| {
            function.blocks.iter().find(|block| {
                matches!(
                    block.instrs.as_slice(),
                    [MirInstr::DefVar { var: v, src, .. }] if *v == var && *src == value
                )
            })
        };
        // The operand is stored before the jump to the header.
        let MirTerm::Jump(header) = function.blocks[0].term else {
            panic!("the comptime for is entered by a jump");
        };
        assert!(matches!(
            function.blocks[0].instrs.last(),
            Some(MirInstr::DefVar {
                var: 1,
                src: Reg(1),
                ..
            })
        ));
        let MirTerm::ComptimeFor {
            body: entry,
            exit: 1,
            ..
        } = function.blocks[header].term
        else {
            panic!("the header exits to the join");
        };
        // The body's entry forwards the argument out of its slot.
        assert!(matches!(
            function.blocks[entry].instrs.first(),
            Some(MirInstr::UseVar {
                dest: Reg(3),
                var: 1,
                mode: UseMode::Forward,
                ..
            })
        ));
        // The continue stores its value and goes back to the header; the
        // stop arm's break, the one that leaves, carries the result, which
        // the join reads as its value.
        let continued = block_storing(1, Reg(2)).expect("the continue's store");
        assert_eq!(jump_target(&continued.term), Some(header));
        assert!(!function.reg_types.contains_key(&4));
        assert_eq!(stored(&function), [Reg(3)]);
    }

    #[test]
    fn a_comptime_for_never_continued_folds_its_arguments() {
        let body = carrying_comptime_for(MirTerm::ComptimeBreak(vec![Reg(3)]));
        let function = lowered(function_over(body, 2));
        // The arguments fold to the operand; the results keep their slot.
        assert_eq!(function.var_names, ["x", "$yield1"]);
        let stores: Vec<Reg> = function
            .blocks
            .iter()
            .flat_map(|block| &block.instrs)
            .filter_map(|instruction| match instruction {
                MirInstr::DefVar { var: 1, src, .. } => Some(*src),
                _ => None,
            })
            .collect();
        assert_eq!(stores, [Reg(1), Reg(1)]);
        assert!(!function.reg_types.contains_key(&3));
        assert_eq!(stored(&function), [Reg(4)]);
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

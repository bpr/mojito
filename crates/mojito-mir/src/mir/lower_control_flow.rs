//! The transitional `lower-control-flow` pass: structured region ops to
//! MIR's CFG, as upstream's `LowerControlFlow` lowers `hlcf` to LLVM blocks.
//!
//! Each block list is rebuilt: the function's, and each `try` part's as its
//! own list. A list's own blocks keep their indices, so an escape target
//! already naming a function block stays valid; a region op's regions are
//! appended after them, and the instructions after the op move to a new join
//! block. An op's results travel through one fresh slot each, stored at each
//! `yield` and forwarded at the join, the shape lowering gives a ternary.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

/// Lower every `if`, `loop`, `comptime if`, and `comptime for` of `function`
/// to blocks and CFG terminators. A body without them is unchanged.
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
    };
    lowering.list(blocks);
    let lowered = lowering.out;
    function.blocks = lowered;
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
}

/// A loop a `break`/`continue` may name: where it continues, where it
/// exits, and the list its blocks are in.
struct Frame {
    kind: FrameKind,
    header: MirBlockId,
    exit: MirBlockId,
    depth: usize,
}

enum FrameKind {
    Loop(Option<LoopLabel>),
    ComptimeFor,
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
                | MirInstr::ComptimeFor { .. } => {
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
            MirInstr::Loop { label, body } => {
                let entry = self.lower_loop(FrameKind::Loop(label), body, join, None);
                (MirTerm::Jump(entry), Vec::new())
            }
            MirInstr::ComptimeIf {
                results,
                conds,
                arms,
                orelse,
            } => self.lower_comptime_if(&results, conds, arms, orelse, join),
            MirInstr::ComptimeFor {
                binder,
                slot,
                source,
                body,
                orelse: _,
            } => {
                let header = self.reserve(1);
                let body = self.lower_loop(FrameKind::ComptimeFor, body, join, Some(header));
                self.out[header].term = MirTerm::ComptimeFor {
                    binder,
                    slot,
                    source,
                    body,
                    exit: join,
                };
                (MirTerm::Jump(header), Vec::new())
            }
            other => (MirTerm::Jump(join), vec![other]),
        }
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
    /// at the body's entry when there is none, and return that entry.
    fn lower_loop(
        &mut self,
        kind: FrameKind,
        body: Vec<MirBlock>,
        join: MirBlockId,
        header: Option<MirBlockId>,
    ) -> MirBlockId {
        let entry = self.reserve(body.len());
        self.frames.push(Frame {
            kind,
            header: header.unwrap_or(entry),
            exit: join,
            depth: self.depth,
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
                instrs.extend(values.iter().zip(slots).map(|(src, var)| MirInstr::DefVar {
                    var: *var,
                    src: *src,
                    binding_ty: None,
                    in_place: false,
                }));
                MirTerm::Jump(*join)
            }
            (MirTerm::ElifCond(cond), RegionEnd::ElifCond { then, next }) => MirTerm::Branch {
                cond,
                then_b: *then,
                else_b: *next,
            },
            (term @ MirTerm::Break { label }, _) => self
                .leave(instrs, |frame| frame.exit, |kind| names_loop(kind, label))
                .unwrap_or(term),
            (term @ MirTerm::Continue { label }, _) => self
                .leave(instrs, |frame| frame.header, |kind| names_loop(kind, label))
                .unwrap_or(term),
            (MirTerm::ComptimeBreak, _) => self
                .leave(instrs, |frame| frame.exit, is_comptime_for)
                .unwrap_or(MirTerm::ComptimeBreak),
            (MirTerm::ComptimeContinue, _) => self
                .leave(instrs, |frame| frame.header, is_comptime_for)
                .unwrap_or(MirTerm::ComptimeContinue),
            (MirTerm::Unreachable, _) => MirTerm::Return(None),
            (term, _) => term,
        }
    }

    /// The edge a `break`/`continue` takes to the innermost frame `names`
    /// accepts: a jump within its list, an escape to the function's list
    /// from a `try` part, and the unsupported boundary from a `try` part
    /// nested in another. `None` when no frame encloses it, which the
    /// verifier rejects.
    fn leave(
        &self,
        instrs: &mut Vec<MirInstr>,
        target: impl Fn(&Frame) -> MirBlockId,
        names: impl Fn(&FrameKind) -> bool,
    ) -> Option<MirTerm> {
        let frame = self.frames.iter().rev().find(|frame| names(&frame.kind))?;
        Some(if frame.depth == self.depth {
            MirTerm::Jump(target(frame))
        } else if frame.depth == 0 {
            MirTerm::EscapeJump {
                target: target(frame),
                cleanup: Vec::new(),
            }
        } else {
            instrs.push(MirInstr::Unsupported(
                "try with break/continue crossing the try boundary".into(),
            ));
            MirTerm::FallOff
        })
    }
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
        })
        .collect()
}

const fn is_comptime_for(kind: &FrameKind) -> bool {
    matches!(kind, FrameKind::ComptimeFor)
}

fn names_loop(kind: &FrameKind, label: Option<LoopLabel>) -> bool {
    match kind {
        FrameKind::Loop(found) => label.is_none() || *found == label,
        FrameKind::ComptimeFor => false,
    }
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
            }]
        ));
        assert!(matches!(join.term, MirTerm::Return(None)));
    }

    #[test]
    fn a_break_from_a_try_part_escapes_to_the_function_list() {
        let body = one(
            vec![MirInstr::Try {
                body: one(Vec::new(), MirTerm::Break { label: None }),
                handler: None,
                orelse: None,
                finalbody: None,
                cleanup: Vec::new(),
            }],
            MirTerm::Continue { label: None },
        );
        let function = lowered(function_with(one(
            vec![MirInstr::Loop { label: None, body }],
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

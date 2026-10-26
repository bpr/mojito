//! `LowerSemanticCF`'s `lowerTryFinally`: a `try`'s `finally` copied before
//! each terminator that leaves the `try`, and at the end of its handler and
//! `else`, so the `try` MIR emits holds no `finally` and every exit runs it
//! as ordinary instructions before the exit.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

/// The parts of a `try` its `finally` is copied into: the body, the
/// handler when it has one, and the `else`, which MIR lowering supplies
/// when the source has none, as upstream's parser stubs it.
pub(super) struct TryParts<'p> {
    pub(super) body: &'p mut Vec<MirBlock>,
    pub(super) handler: Option<&'p mut Vec<MirBlock>>,
    pub(super) orelse: &'p mut Vec<MirBlock>,
}

impl Flatten<'_> {
    /// Copy `finalbody`, the lowered `finally` of the `try` labelled
    /// `label`, before every terminator in `parts` that leaves the `try`,
    /// then at the handler's and the `else`'s own `try.yield`, as
    /// `lowerTryFinally` pastes it. A terminator leaves the `try` when it
    /// is a `return`, a raise naming no `try` entered on the way down from
    /// the part (the `try` itself counts as entered in its body), or a
    /// region exit naming no op entered on the way down, as upstream asks
    /// whether its parent node is a proper ancestor of the `try`. The `try`s
    /// nested in the parts were lowered first, so their `finally`s already
    /// stand before the exits leaving them, ahead of this one. A trivial
    /// `finally` is copied nowhere.
    pub(super) fn lower_try_finally(
        &mut self,
        label: TryLabel,
        finalbody: &[MirBlock],
        parts: TryParts<'_>,
    ) {
        let Some(finally) = finalbody.first().filter(|finally| {
            !finally.instrs.is_empty() || !matches!(finally.term, MirTerm::TryYield(_))
        }) else {
            return;
        };
        let mut paste = Paste { finally, count: 0 };
        let entered = &mut vec![RegionFrame::Try(label)];
        self.paste_before_exits(&mut paste, parts.body, entered);
        for part in parts.handler.into_iter().chain([parts.orelse]) {
            self.paste_before_exits(&mut paste, part, &mut Vec::new());
            if let Some(block) = part
                .first_mut()
                .filter(|block| matches!(block.term, MirTerm::TryYield(_)))
            {
                self.paste_finally(&mut paste, block);
            }
        }
    }

    /// The slot a `try` labelled `label` takes its error in: `binding`,
    /// the `except` clause's, or a fresh `$try_error<N>`, declared here,
    /// before the `try`, as `parseTryStmt` emits `__try_error__`'s
    /// declaration before the `lit.try`.
    pub(super) fn try_error_slot(&mut self, label: TryLabel, binding: Option<VarId>) -> VarId {
        let slot = binding.unwrap_or_else(|| self.var(&format!("$try_error{}", label.0)));
        self.emit(MirInstr::DeclareVar { var: slot });
        slot
    }

    /// The handler a raising `try`/`finally` without an `except` gets, as
    /// upstream's parser synthesizes it in a raising context: it moves the
    /// caught error out of the `try`'s slot `error`, which `body` stores
    /// into, into the nearest error slot outside it, the enclosing `try`'s
    /// or the function's, and raises it there, so the `finally` can be
    /// copied before that raise. The moved register is located at the
    /// first store.
    pub(super) fn reraising_handler(&mut self, error: VarId, body: &[MirBlock]) -> Vec<MirBlock> {
        let ty = self
            .var_types
            .get(&error)
            .cloned()
            .unwrap_or_else(Ty::error);
        let caught = Reg(self.next_reg);
        self.next_reg += 1;
        self.f.reg_types.insert(caught.0, ty);
        let stored =
            nested_instructions(body)
                .into_iter()
                .find_map(|instruction| match instruction {
                    MirInstr::DefVar { var, src, .. } if *var == error => Some(*src),
                    _ => None,
                });
        if let Some((span, _)) = stored.and_then(|src| self.f.spans.0.get(&src.0).cloned()) {
            self.f.spans.0.insert(caught.0, (span, Some(error)));
        }
        let (raise, term) = self.raise_exit(caught);
        let instrs = std::iter::once(MirInstr::UseVar {
            dest: caught,
            var: error,
            mode: UseMode::Move,
            constructor: None,
        })
        .chain(raise)
        .collect();
        vec![MirBlock { instrs, term }]
    }

    /// Paste `finally` before every terminator of `blocks`, at any depth,
    /// that leaves the region ops `entered` lists, each region op adding
    /// the frame its region's exits name while the walk is inside it.
    fn paste_before_exits(
        &mut self,
        paste: &mut Paste<'_>,
        blocks: &mut [MirBlock],
        entered: &mut Vec<RegionFrame>,
    ) {
        for block in blocks {
            for instruction in &mut block.instrs {
                for (frame, region) in framed_regions_mut(instruction) {
                    entered.extend(frame);
                    self.paste_before_exits(paste, region, entered);
                    if frame.is_some() {
                        entered.pop();
                    }
                }
            }
            if leaves(&block.term, entered) {
                self.paste_finally(paste, block);
            }
        }
    }

    /// Paste a copy of the `finally` into `block`, before its terminator,
    /// or before the `lifetime.keep_alive` run a `return` keeps its loop's
    /// iterators alive through: the copy runs while the loop still owns
    /// what it reads. Every copy but the first defines fresh registers, as
    /// upstream mangles every clone's names but the first's. When the
    /// `finally` ends in an exit of its own, that exit replaces the
    /// block's, whose owned return value nothing reads any more and is
    /// discarded first, dying at its definition as upstream's
    /// `CheckLifetimes` destroys it; an error a replaced raise stored
    /// dies in its slot.
    fn paste_finally(&mut self, paste: &mut Paste<'_>, block: &mut MirBlock) {
        let mut copy = paste.finally.clone();
        if paste.count > 0 {
            self.renumber_definitions(&mut copy);
        }
        paste.count += 1;
        let at = if matches!(block.term, MirTerm::Return(_)) {
            block
                .instrs
                .iter()
                .rposition(|instruction| !matches!(instruction, MirInstr::KeepAlive { .. }))
                .map_or(0, |last| last + 1)
        } else {
            block.instrs.len()
        };
        let mut pasted = Vec::new();
        if !matches!(copy.term, MirTerm::TryYield(_)) {
            if let MirTerm::Return(Some(value)) = block.term
                && !self.returns_reference
                && let Some(ty) = self.f.reg_types.get(&value.0).cloned()
            {
                pasted.extend(self.discard_binding(value, ty, false));
            }
            block.term = copy.term;
        }
        pasted.extend(copy.instrs);
        block.instrs.splice(at..at, pasted);
    }

    /// Give every register `copy` defines, at any depth, a fresh number
    /// carrying the original's type and location, as `IRMapping` maps a
    /// cloned op's results.
    fn renumber_definitions(&mut self, copy: &mut MirBlock) {
        let blocks = std::slice::from_mut(copy);
        let mut defined = Vec::new();
        defined_registers(blocks, &mut defined);
        let mut renamed = HashMap::new();
        for register in defined {
            renamed.entry(register.0).or_insert_with(|| {
                let fresh = self.next_reg;
                self.next_reg += 1;
                if let Some(ty) = self.f.reg_types.get(&register.0).cloned() {
                    self.f.reg_types.insert(fresh, ty);
                }
                if let Some(span) = self.f.spans.0.get(&register.0).cloned() {
                    self.f.spans.0.insert(fresh, span);
                }
                fresh
            });
        }
        for_each_register(blocks, &mut |register| {
            if let Some(&fresh) = renamed.get(&register.0) {
                *register = Reg(fresh);
            }
        });
    }
}

/// The `finally` being copied, and how many copies stand.
struct Paste<'f> {
    finally: &'f MirBlock,
    count: usize,
}

/// Whether `term` leaves the region ops `entered` lists: a `return`, a
/// raise out of the function or naming a `try` none of them is, and a
/// region exit naming none of them.
fn leaves(term: &MirTerm, entered: &[RegionFrame]) -> bool {
    let names = |target: RegionFrame| entered.iter().any(|&frame| target.names(frame));
    match term {
        MirTerm::Return(_) | MirTerm::ErrorReturn => true,
        MirTerm::TryRaise { label, .. } => !names(RegionFrame::Try(*label)),
        term => region_exit(term).is_some_and(|(target, _)| !names(target)),
    }
}

/// Each region of `instruction` with the frame its exits name while
/// inside it: a loop's body its loop, a `comptime for`'s body its op, a
/// `match` case its case, and a `try` body its `try`; an `if` arm, a
/// `comptime if` arm, a `match`'s or `comptime for`'s else region, and a
/// `try`'s handler and `else` none.
fn framed_regions_mut(
    instruction: &mut MirInstr,
) -> Vec<(Option<RegionFrame>, &mut Vec<MirBlock>)> {
    match instruction {
        MirInstr::Loop { label, body, .. } => vec![(Some(RegionFrame::Loop(*label)), body)],
        MirInstr::ComptimeFor { body, orelse, .. } => {
            vec![(Some(RegionFrame::ComptimeFor), body), (None, orelse)]
        }
        MirInstr::Match { cases, orelse, .. } => cases
            .iter_mut()
            .map(|case| (Some(RegionFrame::MatchCase), &mut case.body))
            .chain(std::iter::once((None, orelse)))
            .collect(),
        MirInstr::Try {
            label,
            body,
            handler,
            orelse,
            ..
        } => std::iter::once((Some(RegionFrame::Try(*label)), body))
            .chain(handler.iter_mut().map(|blocks| (None, blocks)))
            .chain(orelse.iter_mut().map(|blocks| (None, blocks)))
            .collect(),
        instruction => instruction_regions_mut(instruction)
            .into_iter()
            .map(|region| (None, region))
            .collect(),
    }
}

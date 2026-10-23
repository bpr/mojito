//! The region walks the ownership checks share, each generic over the state
//! it carries.
//!
//! The forward walk is upstream's `UninitializedValueScan` shape in
//! `CheckLifetimes`: an `if` joins its arms (`checkIfOp`), a loop iterates
//! its body until its continue set is stable (`checkLoopOp`), a `match` case
//! starts from its predecessors' `match.next` states (`checkMatchOp`), and a
//! `try`'s handler starts from the states of the raises naming it
//! (`checkTryOp`). The backward walk is `DestructorInsertion`'s shape: a
//! `break` reads its loop's break set, a `continue` its continue set, a
//! loop is dry-run until stable, then replayed once, and a `try` walks its
//! handler and `else` before its body (`checkTryOp`), a raise naming it
//! keeping live what its handler reads. A raise names its `try` as a
//! `break` names its loop, through a frame; a raising call raises through
//! its error region's `raise`, as upstream's `addErrorRegions` makes it.
//! MIR lowering has copied each `finally` before the exits that cross it,
//! as `lowerTryFinally` does, so no walk meets one. Every region is one
//! block.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::{MirCase, RegionFrame, region_exit};

/// A forward analysis the walk carries through regions.
pub(super) trait ForwardScan {
    /// The facts one program point holds.
    type State: Clone + PartialEq;
    /// The state where two paths meet.
    fn join(&self, state: &Self::State, other: &Self::State) -> Self::State;
    /// Apply `instr`'s effects (a region op the walk enters is walked instead).
    fn transfer(&self, state: &mut Self::State, instr: &MirInstr);
    /// The state a `raise` hands the handler.
    fn raise_seed(&self, state: &Self::State) -> Self::State;
    /// Whether the walk enters `instr`'s regions. A region op it does not
    /// enter is observed and transferred as one instruction.
    fn enters(&self, instr: &MirInstr) -> bool;
}

/// A region's states per exit channel: the normal fall-off, every raise
/// out of the function, and `return` exits. `None` marks an unreachable
/// channel. The states a `break`, `continue`, `match.next`,
/// `match.complete`, or raise naming a `try` carries go to their [`Frame`]
/// instead.
pub(super) struct Flow<T> {
    pub(super) normal: Option<T>,
    pub(super) raises: Option<T>,
    pub(super) exits: Option<T>,
}

/// What a forward walk reports as it replays a body: every instruction it
/// transfers, every region op it enters, and every block it reaches, each
/// exactly once. A closure over `(state, instruction)` observes the
/// instructions alone.
pub(super) trait ForwardObserver<T> {
    /// An instruction the walk transfers, with the state reaching it.
    fn instr(&mut self, state: &T, instr: &MirInstr) -> Result<(), OwnershipError>;
    /// A region op the walk enters, with the state reaching it.
    fn op(&mut self, _state: &T, _instr: &MirInstr) {}
    /// A block the walk reaches, with the state entering it and the state
    /// at its terminator (`None` when its end is unreachable).
    fn block(&mut self, _entry: &T, _end: Option<&T>, _block: &MirBlock) {}
}

impl<T, F> ForwardObserver<T> for F
where
    F: FnMut(&T, &MirInstr) -> Result<(), OwnershipError>,
{
    fn instr(&mut self, state: &T, instr: &MirInstr) -> Result<(), OwnershipError> {
        self(state, instr)
    }
}

/// Walk a function body forward from `entry`, handing every instruction and
/// the state reaching it, before its effects, to `observer` exactly once.
pub(super) fn walk_forward<S, O>(
    scan: &S,
    entry: S::State,
    blocks: &[MirBlock],
    observer: &mut O,
) -> Result<Flow<S::State>, OwnershipError>
where
    S: ForwardScan,
    O: ForwardObserver<S::State>,
{
    walk_region(scan, Some(entry), blocks, Some(observer), &mut Vec::new())
}

/// A backward analysis the walk carries through regions.
pub(super) trait BackwardScan {
    /// The facts live at one program point; `Default` is nothing live.
    type State: Clone + PartialEq + Default;
    /// Add `other`'s facts to `state`.
    fn join(&self, state: &mut Self::State, other: &Self::State);
    /// Undo `instr`: kill what it defines, then add what it uses.
    fn transfer(&self, state: &mut Self::State, instr: &MirInstr);
    /// Whether the walk enters `instr`'s regions. A region op it does not
    /// enter is transferred as one instruction.
    fn enters(&self, instr: &MirInstr) -> bool;
    /// Kill `var`, which the entry of a region defines: a handler's error
    /// binding, which the caught error arrives in.
    fn define(&self, state: &mut Self::State, var: VarId);
    /// Add to `live`, the state live after `block`'s terminator, what the
    /// terminator itself reads.
    fn terminate(&self, _live: &mut Self::State, _block: &MirBlock) {}
    /// `live`, the live-in of a `try` part, as the `try` sees it at the
    /// part's entry.
    fn enter(&self, _live: &mut Self::State, _part: &[MirBlock]) {}
}

/// What a backward walk reports on its final replay, each event once. A
/// closure over `(instruction, state)` records the instructions alone.
pub(super) trait BackwardRecorder<T> {
    /// An instruction with the state live just before it; a region op is
    /// reported once its regions are walked.
    fn instr(&mut self, _instr: &MirInstr, _live: &T) {}
    /// A block with the states live entering it and after its terminator.
    fn block(&mut self, _block: &MirBlock, _live: BlockLiveness<'_, T>) {}
    /// A `try` with the states its parts were walked from.
    fn try_op(&mut self, _op: &MirInstr, _channels: &TryChannels<T>) {}
}

impl<T, F: FnMut(&MirInstr, &T)> BackwardRecorder<T> for F {
    fn instr(&mut self, instr: &MirInstr, live: &T) {
        self(instr, live);
    }
}

/// The states live around one block, as [`BackwardRecorder::block`]
/// reports them.
pub(super) struct BlockLiveness<'a, T> {
    pub(super) live_in: &'a T,
    pub(super) live_out: &'a T,
}

/// The states one `try`'s parts were walked from, as `checkTryOp` walks
/// them: the handler and `else` from the `try`'s live-out, and the body
/// from the `else`'s (or the live-out).
#[derive(Clone)]
pub(super) struct TryChannels<T> {
    /// The handler's live-in, its error binding included.
    pub(super) handler: Option<T>,
}

/// Walk a function body backward from its exits, handing `recorder` each
/// instruction with the state live just before it, each block with the
/// states around it, and each `try` with its channels, exactly once. The
/// walk enters every region op `scan` enters: a raise naming a `try` keeps
/// live what its handler reads, and a `return` or a raise out of the
/// function nothing.
pub(super) fn walk_backward<S, R>(scan: &S, blocks: &[MirBlock], recorder: &mut R) -> S::State
where
    S: BackwardScan,
    R: BackwardRecorder<S::State>,
{
    let mut walk = BackWalk {
        scan,
        frames: Vec::new(),
    };
    walk.region(blocks, &S::State::default(), Some(recorder))
}

/// Whether `instr` is a region op the walks know.
pub(super) const fn region_op(instr: &MirInstr) -> bool {
    matches!(
        instr,
        MirInstr::Try { .. }
            | MirInstr::If { .. }
            | MirInstr::Loop { .. }
            | MirInstr::ComptimeIf { .. }
            | MirInstr::ComptimeFor { .. }
            | MirInstr::Match { .. }
    )
}

impl<T: Clone> Flow<T> {
    const fn unreachable() -> Self {
        Self {
            normal: None,
            raises: None,
            exits: None,
        }
    }

    /// A flow that falls off with `state` and leaves no other way.
    const fn falls_off(state: T) -> Self {
        Self {
            normal: Some(state),
            raises: None,
            exits: None,
        }
    }

    /// Join every channel of `other` into this flow's.
    fn absorb<S: ForwardScan<State = T>>(&mut self, scan: &S, other: &Self) {
        add_state(scan, &mut self.normal, other.normal.as_ref());
        self.absorb_abnormal(scan, other);
    }

    /// Join the raising and exiting channels of `other` into this flow's.
    fn absorb_abnormal<S: ForwardScan<State = T>>(&mut self, scan: &S, other: &Self) {
        add_state(scan, &mut self.raises, other.raises.as_ref());
        add_state(scan, &mut self.exits, other.exits.as_ref());
    }
}

/// An op a region terminator inside it leaves to, upstream's break and
/// continue sets of a loop and the next and complete sets of a `match`
/// entry: the states joined at each of its two exits.
struct Frame<T> {
    kind: RegionFrame,
    /// A loop's break set; a `match` case's complete set.
    leave: Option<T>,
    /// A loop's continue set; a `match` case's next set.
    resume: Option<T>,
}

impl<T> Frame<T> {
    const fn new(kind: RegionFrame) -> Self {
        Self {
            kind,
            leave: None,
            resume: None,
        }
    }
}

/// A region's flow beside the frame its terminators filled.
type Framed<T> = (Flow<T>, Frame<T>);

/// Join a channel state into `target` (unreachable contributes nothing).
fn add_state<S: ForwardScan>(scan: &S, target: &mut Option<S::State>, source: Option<&S::State>) {
    match (target.as_mut(), source) {
        (_, None) => {}
        (None, Some(source)) => *target = Some(source.clone()),
        (Some(target), Some(source)) => *target = scan.join(target, source),
    }
}

/// Whether `instr` is a region op the forward walk knows, and `scan` enters it.
fn enters<S: ForwardScan>(scan: &S, instr: &MirInstr) -> bool {
    region_op(instr) && scan.enters(instr)
}

/// Walk one region, its one block, from its entry state, handing every
/// instruction and the state reaching it to `observer` when one is given.
/// Region ops recurse through [`walk_op`], and a region terminator hands
/// its state to the frame it names.
fn walk_region<S, O>(
    scan: &S,
    entry: Option<S::State>,
    blocks: &[MirBlock],
    observer: Option<&mut O>,
    frames: &mut Vec<Frame<S::State>>,
) -> Result<Flow<S::State>, OwnershipError>
where
    S: ForwardScan,
    O: ForwardObserver<S::State>,
{
    let Some(entry) = entry else {
        return Ok(Flow::unreachable());
    };
    let Some(block) = blocks.first() else {
        return Ok(Flow::falls_off(entry));
    };
    let mut flow = Flow::unreachable();
    let Some(state) = walk_block(scan, entry, block, &mut flow, observer, frames)? else {
        return Ok(flow);
    };
    match block.term {
        MirTerm::TryYield(_)
        | MirTerm::Yield(_)
        | MirTerm::ComptimeYield(_)
        | MirTerm::ElifCond { .. } => add_state(scan, &mut flow.normal, Some(&state)),
        MirTerm::Return(_) => add_state(scan, &mut flow.exits, Some(&state)),
        MirTerm::Raise { label: None, .. } => {
            let seed = scan.raise_seed(&state);
            add_state(scan, &mut flow.raises, Some(&seed));
        }
        MirTerm::Raise {
            label: Some(label), ..
        } => {
            let seed = scan.raise_seed(&state);
            leave_to(scan, frames, RegionFrame::Try(label), false, &seed);
        }
        MirTerm::Break { .. }
        | MirTerm::Continue { .. }
        | MirTerm::ComptimeBreak(_)
        | MirTerm::ComptimeContinue(_)
        | MirTerm::MatchComplete(_)
        | MirTerm::MatchNext(_) => {
            if let Some((target, resume)) = region_exit(&block.term) {
                leave_to(scan, frames, target, resume, &state);
            }
        }
        // `hlcf.unreachable` checks nothing: no value reaches past it.
        MirTerm::Unreachable => {}
    }
    Ok(flow)
}

/// Walk one block's instructions from `state`, collecting their raise
/// states and their nested ops' abnormal channels into `flow`. `None` when
/// the block's end is unreachable (after a region op that never falls
/// off).
fn walk_block<S, O>(
    scan: &S,
    state: S::State,
    block: &MirBlock,
    flow: &mut Flow<S::State>,
    mut observer: Option<&mut O>,
    frames: &mut Vec<Frame<S::State>>,
) -> Result<Option<S::State>, OwnershipError>
where
    S: ForwardScan,
    O: ForwardObserver<S::State>,
{
    let entry = observer.is_some().then(|| state.clone());
    let mut current = Some(state);
    for instr in &block.instrs {
        let Some(mut state) = current.take() else {
            break;
        };
        if enters(scan, instr) {
            if let Some(observer) = observer.as_deref_mut() {
                observer.op(&state, instr);
            }
            let nested = walk_op(scan, state, instr, observer.as_deref_mut(), frames)?;
            flow.absorb_abnormal(scan, &nested);
            current = nested.normal;
            continue;
        }
        if let Some(observer) = observer.as_deref_mut() {
            observer.instr(&state, instr)?;
        }
        scan.transfer(&mut state, instr);
        current = Some(state);
    }
    if let (Some(observer), Some(entry)) = (observer, &entry) {
        observer.block(entry, current.as_ref(), block);
    }
    Ok(current)
}

/// Walk one region op from `entry`, as `scanBlock` dispatches it.
fn walk_op<S, O>(
    scan: &S,
    entry: S::State,
    instr: &MirInstr,
    observer: Option<&mut O>,
    frames: &mut Vec<Frame<S::State>>,
) -> Result<Flow<S::State>, OwnershipError>
where
    S: ForwardScan,
    O: ForwardObserver<S::State>,
{
    match instr {
        MirInstr::Try { .. } => walk_try(scan, entry, instr, observer, frames),
        MirInstr::If { .. } => walk_if(scan, entry, instr, observer, frames),
        MirInstr::ComptimeIf { arms, orelse, .. } => walk_arms(
            scan,
            &entry,
            arms.iter().chain(std::iter::once(orelse)),
            observer,
            frames,
        ),
        MirInstr::Loop { label, body, .. } => walk_loop(
            scan,
            entry,
            RegionFrame::Loop(*label),
            body,
            observer,
            frames,
        ),
        // The else region is `unreachable` once `LowerSemanticCF` has run.
        MirInstr::ComptimeFor { body, .. } => walk_loop(
            scan,
            entry,
            RegionFrame::ComptimeFor,
            body,
            observer,
            frames,
        ),
        MirInstr::Match { cases, orelse, .. } => {
            walk_match(scan, entry, cases, orelse, observer, frames)
        }
        _ => Ok(Flow::falls_off(entry)),
    }
}

/// Join `state` into the innermost frame `target` names: into its resume
/// set when `resume`, else into its leave set.
fn leave_to<S: ForwardScan>(
    scan: &S,
    frames: &mut [Frame<S::State>],
    target: RegionFrame,
    resume: bool,
    state: &S::State,
) {
    if let Some(frame) = frames
        .iter_mut()
        .rev()
        .find(|frame| target.names(frame.kind))
    {
        let set = if resume {
            &mut frame.resume
        } else {
            &mut frame.leave
        };
        add_state(scan, set, Some(state));
    }
}

/// Walk `blocks` from `entry` under a fresh frame of `kind`, returning the
/// region's flow and the frame its terminators filled.
fn walk_framed<S, O>(
    scan: &S,
    entry: S::State,
    kind: RegionFrame,
    blocks: &[MirBlock],
    observer: Option<&mut O>,
    frames: &mut Vec<Frame<S::State>>,
) -> Result<Framed<S::State>, OwnershipError>
where
    S: ForwardScan,
    O: ForwardObserver<S::State>,
{
    let depth = frames.len();
    frames.push(Frame::new(kind));
    let flow = walk_region(scan, Some(entry), blocks, observer, frames)?;
    Ok((flow, frames.remove(depth)))
}

/// Walk an `if`, as `checkIfOp`: the then arm from `entry`, each elif's
/// condition from the state the previous condition left and its arm from
/// the state after it, and the else arm from the state the last condition
/// left. Without elifs this is `checkIfLikeOp`. The live-out is the join of
/// every arm's.
fn walk_if<S, O>(
    scan: &S,
    entry: S::State,
    instr: &MirInstr,
    mut observer: Option<&mut O>,
    frames: &mut Vec<Frame<S::State>>,
) -> Result<Flow<S::State>, OwnershipError>
where
    S: ForwardScan,
    O: ForwardObserver<S::State>,
{
    let MirInstr::If {
        then,
        elifs,
        orelse,
        ..
    } = instr
    else {
        return Ok(Flow::falls_off(entry));
    };
    let mut flow = walk_region(
        scan,
        Some(entry.clone()),
        then,
        observer.as_deref_mut(),
        frames,
    )?;
    let mut rest = Some(entry);
    for elif in elifs {
        let cond = walk_region(scan, rest, &elif.cond, observer.as_deref_mut(), frames)?;
        flow.absorb_abnormal(scan, &cond);
        let arm = walk_region(
            scan,
            cond.normal.clone(),
            &elif.then,
            observer.as_deref_mut(),
            frames,
        )?;
        flow.absorb(scan, &arm);
        rest = cond.normal;
    }
    let arm = walk_region(scan, rest, orelse, observer, frames)?;
    flow.absorb(scan, &arm);
    Ok(flow)
}

/// Walk each of `regions` from `entry`, as `checkIfLikeOp` walks a
/// `comptime if`'s arms: the live-out is the join of every arm's.
fn walk_arms<'r, S, O>(
    scan: &S,
    entry: &S::State,
    regions: impl Iterator<Item = &'r Vec<MirBlock>>,
    mut observer: Option<&mut O>,
    frames: &mut Vec<Frame<S::State>>,
) -> Result<Flow<S::State>, OwnershipError>
where
    S: ForwardScan,
    O: ForwardObserver<S::State>,
{
    let mut flow = Flow::unreachable();
    for region in regions {
        let arm = walk_region(
            scan,
            Some(entry.clone()),
            region,
            observer.as_deref_mut(),
            frames,
        )?;
        flow.absorb(scan, &arm);
    }
    Ok(flow)
}

/// Walk a loop (or a `comptime for`) as `checkLoopOp`: its continue set
/// starts as `entry`, and the body is walked from it until the states its
/// `continue`s join in stop changing; the break set is the live-out. Only
/// the walk from the converged continue set is observed.
fn walk_loop<S, O>(
    scan: &S,
    entry: S::State,
    kind: RegionFrame,
    body: &[MirBlock],
    observer: Option<&mut O>,
    frames: &mut Vec<Frame<S::State>>,
) -> Result<Flow<S::State>, OwnershipError>
where
    S: ForwardScan,
    O: ForwardObserver<S::State>,
{
    let mut resume = entry;
    let (mut flow, mut frame) = loop {
        let (flow, frame) = walk_framed(scan, resume.clone(), kind, body, None::<&mut O>, frames)?;
        let mut next = Some(resume.clone());
        add_state(scan, &mut next, frame.resume.as_ref());
        match next {
            Some(next) if next != resume => resume = next,
            _ => break (flow, frame),
        }
    };
    if observer.is_some() {
        (flow, frame) = walk_framed(scan, resume, kind, body, observer, frames)?;
    }
    Ok(Flow {
        normal: frame.leave,
        ..flow
    })
}

/// Walk a `match` as `checkMatchOp`: each case from the states the previous
/// case's `match.next`s joined (the first from `entry`), and the else
/// region from the last case's, outside this match's frame so a
/// `match.next`/`match.complete` there resolves to an enclosing one. The
/// live-out is the join of every `match.complete` state and the else
/// region's.
fn walk_match<S, O>(
    scan: &S,
    entry: S::State,
    cases: &[MirCase],
    orelse: &[MirBlock],
    mut observer: Option<&mut O>,
    frames: &mut Vec<Frame<S::State>>,
) -> Result<Flow<S::State>, OwnershipError>
where
    S: ForwardScan,
    O: ForwardObserver<S::State>,
{
    let mut flow = Flow::unreachable();
    let mut next = Some(entry);
    for case in cases {
        let Some(entry) = next else {
            break;
        };
        let (case_flow, frame) = walk_framed(
            scan,
            entry,
            RegionFrame::MatchCase,
            &case.body,
            observer.as_deref_mut(),
            frames,
        )?;
        flow.absorb_abnormal(scan, &case_flow);
        add_state(scan, &mut flow.normal, frame.leave.as_ref());
        next = frame.resume;
    }
    let else_flow = walk_region(scan, next, orelse, observer, frames)?;
    flow.absorb(scan, &else_flow);
    Ok(flow)
}

/// Walk the regions of one `try`, as `checkTryOp`: the body from `entry`
/// under the frame its raises name, `else` from the body's normal
/// completion, and the handler from the states the raises naming the `try`
/// carry. The live-out is the join of the handler's and the `else`'s (or
/// the body's) fall-offs.
fn walk_try<S, O>(
    scan: &S,
    entry: S::State,
    instr: &MirInstr,
    mut observer: Option<&mut O>,
    frames: &mut Vec<Frame<S::State>>,
) -> Result<Flow<S::State>, OwnershipError>
where
    S: ForwardScan,
    O: ForwardObserver<S::State>,
{
    let MirInstr::Try {
        label,
        body,
        handler,
        orelse,
        ..
    } = instr
    else {
        return Ok(Flow::falls_off(entry));
    };
    let kind = RegionFrame::Try(*label);
    let (body_flow, caught) =
        walk_framed(scan, entry, kind, body, observer.as_deref_mut(), frames)?;
    let mut flow = Flow {
        normal: None,
        raises: body_flow.raises,
        exits: body_flow.exits,
    };
    match orelse {
        Some(orelse) => {
            let else_flow = walk_region(
                scan,
                body_flow.normal,
                orelse,
                observer.as_deref_mut(),
                frames,
            )?;
            flow.absorb(scan, &else_flow);
        }
        None => add_state(scan, &mut flow.normal, body_flow.normal.as_ref()),
    }
    if let Some((_, handler)) = handler {
        let handler_flow = walk_region(scan, caught.leave, handler, observer, frames)?;
        flow.absorb(scan, &handler_flow);
    }
    Ok(flow)
}

/// A frame of the backward walk: the states live where its two exits go.
struct BackFrame<T> {
    kind: RegionFrame,
    leave: T,
    resume: T,
}

/// One backward walk: the frames enclosing the region being walked, a
/// `try` body's leave set the live-in of its handler.
struct BackWalk<'s, S: BackwardScan> {
    scan: &'s S,
    frames: Vec<BackFrame<S::State>>,
}

impl<S: BackwardScan> BackWalk<'_, S> {
    /// Walk one region, its one block, backward from `exit`, the state its
    /// yields continue to, returning its live-in.
    fn region<R: BackwardRecorder<S::State>>(
        &mut self,
        blocks: &[MirBlock],
        exit: &S::State,
        recorder: Option<&mut R>,
    ) -> S::State {
        blocks
            .first()
            .map_or_else(|| exit.clone(), |block| self.block(block, exit, recorder))
    }

    /// Walk one block from its terminator back, returning its live-in.
    fn block<R: BackwardRecorder<S::State>>(
        &mut self,
        block: &MirBlock,
        exit: &S::State,
        mut recorder: Option<&mut R>,
    ) -> S::State {
        let out = self.live_out(&block.term, exit);
        let mut live = out.clone();
        self.scan.terminate(&mut live, block);
        for instr in block.instrs.iter().rev() {
            if region_op(instr) && self.scan.enters(instr) {
                live = self.op(instr, &live, recorder.as_deref_mut());
            } else {
                self.scan.transfer(&mut live, instr);
                if raises_to_handler(instr)
                    && let Some(handler) = self
                        .frames
                        .iter()
                        .rev()
                        .find(|frame| matches!(frame.kind, RegionFrame::Try(_)))
                {
                    self.scan.join(&mut live, &handler.leave);
                }
            }
            if let Some(recorder) = recorder.as_deref_mut() {
                recorder.instr(instr, &live);
            }
        }
        if let Some(recorder) = recorder {
            recorder.block(
                block,
                BlockLiveness {
                    live_in: &live,
                    live_out: &out,
                },
            );
        }
        live
    }

    /// The state live after a block's terminator: `exit` after a yield,
    /// nothing after a `return` or a raise out of the function, and the
    /// named frame's set after a region terminator or a raise naming a
    /// `try`, its handler's live-in.
    fn live_out(&self, term: &MirTerm, exit: &S::State) -> S::State {
        match term {
            MirTerm::TryYield(_)
            | MirTerm::Yield(_)
            | MirTerm::ComptimeYield(_)
            | MirTerm::ElifCond { .. } => exit.clone(),
            MirTerm::Return(_) | MirTerm::Raise { label: None, .. } | MirTerm::Unreachable => {
                S::State::default()
            }
            MirTerm::Raise { .. }
            | MirTerm::Break { .. }
            | MirTerm::Continue { .. }
            | MirTerm::ComptimeBreak(_)
            | MirTerm::ComptimeContinue(_)
            | MirTerm::MatchComplete(_)
            | MirTerm::MatchNext(_) => {
                let target = match term {
                    MirTerm::Raise {
                        label: Some(label), ..
                    } => Some((RegionFrame::Try(*label), false)),
                    term => region_exit(term),
                };
                let Some((target, resume)) = target else {
                    return S::State::default();
                };
                self.frames
                    .iter()
                    .rev()
                    .find(|frame| target.names(frame.kind))
                    .map(|frame| {
                        if resume {
                            frame.resume.clone()
                        } else {
                            frame.leave.clone()
                        }
                    })
                    .unwrap_or_default()
            }
        }
    }

    /// The live-in of one region op whose live-out is `exit`.
    ///
    /// - An `if`: the else arm from `exit`, then each elif in reverse, its
    ///   arm from `exit` and its condition from the join of that arm's
    ///   live-in and the rest's; the then arm's live-in joins the rest's.
    /// - A `comptime if`: the join of every arm's live-in.
    /// - A loop or `comptime for`: see [`Self::looped`].
    /// - A `match`: the else region from `exit`, outside this match's frame,
    ///   then each case in reverse under a frame whose next set is the
    ///   following case's live-in and whose complete set is `exit`.
    /// - A `try`: see [`Self::tried`].
    fn op<R: BackwardRecorder<S::State>>(
        &mut self,
        instr: &MirInstr,
        exit: &S::State,
        mut recorder: Option<&mut R>,
    ) -> S::State {
        match instr {
            MirInstr::If {
                then,
                elifs,
                orelse,
                ..
            } => {
                let mut rest = self.region(orelse, exit, recorder.as_deref_mut());
                for elif in elifs.iter().rev() {
                    let arm = self.region(&elif.then, exit, recorder.as_deref_mut());
                    self.scan.join(&mut rest, &arm);
                    rest = self.region(&elif.cond, &rest, recorder.as_deref_mut());
                }
                let mut live = self.region(then, exit, recorder);
                self.scan.join(&mut live, &rest);
                live
            }
            MirInstr::ComptimeIf { arms, orelse, .. } => {
                let mut live = S::State::default();
                for arm in arms.iter().chain(std::iter::once(orelse)) {
                    let arm = self.region(arm, exit, recorder.as_deref_mut());
                    self.scan.join(&mut live, &arm);
                }
                live
            }
            MirInstr::Loop { label, body, .. } => {
                self.looped(RegionFrame::Loop(*label), body, exit, recorder)
            }
            MirInstr::ComptimeFor { body, .. } => {
                self.looped(RegionFrame::ComptimeFor, body, exit, recorder)
            }
            MirInstr::Match { cases, orelse, .. } => {
                let mut next = self.region(orelse, exit, recorder.as_deref_mut());
                for case in cases.iter().rev() {
                    self.frames.push(BackFrame {
                        kind: RegionFrame::MatchCase,
                        leave: exit.clone(),
                        resume: next,
                    });
                    next = self.region(&case.body, &S::State::default(), recorder.as_deref_mut());
                    self.frames.pop();
                }
                next
            }
            MirInstr::Try { .. } => self.tried(instr, exit, recorder),
            _ => {
                let mut live = exit.clone();
                self.scan.transfer(&mut live, instr);
                live
            }
        }
    }

    /// The live-in of a loop (or a `comptime for`) whose live-out is
    /// `exit`, as `checkLoopOp` walks it: the body is dry-run under a frame
    /// whose break set is `exit` and whose continue set starts empty, until
    /// the continue set (the body's live-in) is stable, then replayed once
    /// for `recorder`.
    fn looped<R: BackwardRecorder<S::State>>(
        &mut self,
        kind: RegionFrame,
        body: &[MirBlock],
        exit: &S::State,
        recorder: Option<&mut R>,
    ) -> S::State {
        let mut resume = S::State::default();
        loop {
            let mut next = self.framed(kind, body, exit, resume.clone(), None::<&mut R>);
            self.scan.join(&mut next, &resume);
            if next == resume {
                break;
            }
            resume = next;
        }
        if recorder.is_some() {
            self.framed(kind, body, exit, resume.clone(), recorder);
        }
        resume
    }

    /// Walk a loop body under a frame of `kind` whose break set is `exit`
    /// and whose continue set is `resume`.
    fn framed<R: BackwardRecorder<S::State>>(
        &mut self,
        kind: RegionFrame,
        body: &[MirBlock],
        exit: &S::State,
        resume: S::State,
        recorder: Option<&mut R>,
    ) -> S::State {
        self.frames.push(BackFrame {
            kind,
            leave: exit.clone(),
            resume,
        });
        let live = self.region(body, &S::State::default(), recorder);
        self.frames.pop();
        live
    }

    /// The live-in of a `try` whose live-out is `exit`, as `checkTryOp`
    /// walks it: the handler and `else` from `exit`, then the body from the
    /// `else`'s live-in (or `exit`) under a frame whose leave set is the
    /// handler's live-in, its error binding defined, which a raise naming
    /// the `try` keeps live.
    fn tried<R: BackwardRecorder<S::State>>(
        &mut self,
        instr: &MirInstr,
        exit: &S::State,
        mut recorder: Option<&mut R>,
    ) -> S::State {
        let MirInstr::Try {
            label,
            body,
            handler,
            orelse,
            ..
        } = instr
        else {
            return exit.clone();
        };
        let handler_in = handler.as_ref().map(|(_, h)| {
            let mut live = self.region(h, exit, recorder.as_deref_mut());
            self.scan.enter(&mut live, h);
            live
        });
        let else_in = orelse.as_ref().map(|e| {
            let mut live = self.region(e, exit, recorder.as_deref_mut());
            self.scan.enter(&mut live, e);
            live
        });
        let mut caught = handler_in.clone().unwrap_or_default();
        if let Some((Some(binding), _)) = handler {
            self.scan.define(&mut caught, *binding);
        }
        self.frames.push(BackFrame {
            kind: RegionFrame::Try(*label),
            leave: caught,
            resume: S::State::default(),
        });
        let mut live = self.region(
            body,
            else_in.as_ref().unwrap_or(exit),
            recorder.as_deref_mut(),
        );
        self.frames.pop();
        self.scan.enter(&mut live, body);
        if let Some(recorder) = recorder {
            recorder.try_op(
                instr,
                &TryChannels {
                    handler: handler_in,
                },
            );
        }
        live
    }
}

//! The region walks the ownership checks share, each generic over the state
//! it carries.
//!
//! The forward walk is upstream's `UninitializedValueScan` shape in
//! `CheckLifetimes`: an `if` joins its arms (`checkIfOp`), a loop iterates
//! its body until its continue set is stable (`checkLoopOp`), a `match` case
//! starts from its predecessors' `match.next` states (`checkMatchOp`), and a
//! `try` walks its four parts with the raise-point rule (`checkTryOp`). The
//! backward walk is `DestructorInsertion`'s shape: a `break` reads its loop's
//! break set, a `continue` its continue set, a loop is dry-run until
//! stable, then replayed once, and a `try` walks its parts from its
//! `finally` back to its body (`checkTryOp`), a raising instruction keeping
//! live what its handler reads. A lowered block list (MIR text, or a `try`
//! part below `lower-control-flow`) reaches its per-block fixpoint first.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::{LoopLabel, MirBlockId};

/// A forward analysis the walk carries through regions.
pub(super) trait ForwardScan {
    /// The facts one program point holds.
    type State: Clone + PartialEq;
    /// The state where two paths meet.
    fn join(&self, state: &Self::State, other: &Self::State) -> Self::State;
    /// Apply `instr`'s effects (a region op the walk enters is walked instead).
    fn transfer(&self, state: &mut Self::State, instr: &MirInstr);
    /// The state a raise at the instruction just transferred hands the
    /// handler; `next` is the instruction after it.
    fn raise_seed(&self, state: &Self::State, next: Option<&MirInstr>) -> Self::State;
    /// Whether the walk enters `instr`'s regions. A region op it does not
    /// enter is observed and transferred as one instruction.
    fn enters(&self, instr: &MirInstr) -> bool;
}

/// A region's states per exit channel: the normal fall-off, every
/// potentially-raising point, and `return` exits. `None` marks an
/// unreachable channel. The states a `break`, `continue`, `match.next`, or
/// `match.complete` carries go to their [`Frame`] instead; in a lowered
/// body, the state an escape from a `try` part carries is kept with the
/// function block it enters, which the function's fixpoint joins it into.
pub(super) struct Flow<T> {
    pub(super) normal: Option<T>,
    pub(super) raises: Option<T>,
    pub(super) exits: Option<T>,
    escapes: Vec<(MirBlockId, T)>,
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
    /// A block with the states live entering it and after its terminator,
    /// and the state a raise in it leaves live (`None` outside every `try`).
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
    pub(super) raise: Option<&'a T>,
}

/// The states one `try`'s parts were walked from, as `checkTryOp` walks
/// them with Mojito's `finally` kept: the `finally` from the `try`'s
/// live-out, the handler and `else` from the `finally`'s live-in, and the
/// body from the `else`'s (or the `finally`'s).
#[derive(Clone)]
pub(super) struct TryChannels<T> {
    /// Live after the `try`.
    pub(super) after: T,
    /// Live entering the `finally` on the normal path; `after` without one.
    pub(super) fin: T,
    /// What a `return` in the body, handler, or `else` leaves live: the
    /// `finally`'s live-in on the return path, or the enclosing return's.
    pub(super) returns: T,
    /// The handler's live-in, its error binding included.
    pub(super) handler: Option<T>,
}

/// Walk a function body backward from its exits, handing `recorder` each
/// instruction with the state live just before it, each block with the
/// states around it, and each `try` with its channels, exactly once. The
/// walk enters every region op `scan` enters: a raising instruction inside
/// a `try` keeps live what the raise's handler (or `finally`) reads, and a
/// `return` what the `finally`s it crosses read.
pub(super) fn walk_backward<S, R>(scan: &S, blocks: &[MirBlock], recorder: &mut R) -> S::State
where
    S: BackwardScan,
    R: BackwardRecorder<S::State>,
{
    let mut walk = BackWalk {
        scan,
        frames: Vec::new(),
        raise: None,
        exit: S::State::default(),
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
            escapes: Vec::new(),
        }
    }

    /// A flow that falls off with `state` and leaves no other way.
    const fn falls_off(state: T) -> Self {
        Self {
            normal: Some(state),
            raises: None,
            exits: None,
            escapes: Vec::new(),
        }
    }

    /// Join every channel of `other` into this flow's.
    fn absorb<S: ForwardScan<State = T>>(&mut self, scan: &S, other: &Self) {
        add_state(scan, &mut self.normal, other.normal.as_ref());
        self.absorb_abnormal(scan, other);
    }

    /// Join the raising, exiting, and escaping channels of `other` into
    /// this flow's.
    fn absorb_abnormal<S: ForwardScan<State = T>>(&mut self, scan: &S, other: &Self) {
        add_state(scan, &mut self.raises, other.raises.as_ref());
        add_state(scan, &mut self.exits, other.exits.as_ref());
        self.escapes.extend(other.escapes.iter().cloned());
    }
}

/// Which region terminators a frame resolves, by the rules
/// `lower-control-flow` resolves them by: a `break`/`continue` names the
/// nearest loop carrying its label (any loop when it has none),
/// `comptime.for.break`/`continue` the nearest `comptime for`, and
/// `match.next`/`match.complete` the nearest `match` case.
#[derive(Clone, Copy)]
enum FrameKind {
    Loop(Option<LoopLabel>),
    ComptimeFor,
    MatchCase,
}

/// An op a region terminator inside it leaves to, upstream's break and
/// continue sets of a loop and the next and complete sets of a `match`
/// entry: the states joined at each of its two exits.
struct Frame<T> {
    kind: FrameKind,
    /// A loop's break set; a `match` case's complete set.
    leave: Option<T>,
    /// A loop's continue set; a `match` case's next set.
    resume: Option<T>,
}

impl<T> Frame<T> {
    const fn new(kind: FrameKind) -> Self {
        Self {
            kind,
            leave: None,
            resume: None,
        }
    }
}

/// A region's flow beside the frame its terminators filled.
type Framed<T> = (Flow<T>, Frame<T>);

/// The observer type of a replay that observes nothing (a fixpoint pass).
type NoObserver<T> = fn(&T, &MirInstr) -> Result<(), OwnershipError>;

/// Join a channel state into `target` (unreachable contributes nothing).
fn add_state<S: ForwardScan>(scan: &S, target: &mut Option<S::State>, source: Option<&S::State>) {
    match (target.as_mut(), source) {
        (_, None) => {}
        (None, Some(source)) => *target = Some(source.clone()),
        (Some(target), Some(source)) => *target = scan.join(target, source),
    }
}

/// The frame a region terminator leaves to, as the frame kind it names, and
/// whether it resumes that frame (`continue`, `match.next`) or leaves it.
const fn region_exit(term: &MirTerm) -> Option<(FrameKind, bool)> {
    match term {
        MirTerm::Break { label } => Some((FrameKind::Loop(*label), false)),
        MirTerm::Continue { label } => Some((FrameKind::Loop(*label), true)),
        MirTerm::ComptimeBreak => Some((FrameKind::ComptimeFor, false)),
        MirTerm::ComptimeContinue => Some((FrameKind::ComptimeFor, true)),
        MirTerm::MatchComplete => Some((FrameKind::MatchCase, false)),
        MirTerm::MatchNext => Some((FrameKind::MatchCase, true)),
        _ => None,
    }
}

/// Whether a terminator naming `target` resolves to a frame of `kind`.
fn names(target: FrameKind, kind: FrameKind) -> bool {
    match (target, kind) {
        (FrameKind::Loop(label), FrameKind::Loop(found)) => label.is_none() || found == label,
        (FrameKind::ComptimeFor, FrameKind::ComptimeFor)
        | (FrameKind::MatchCase, FrameKind::MatchCase) => true,
        _ => false,
    }
}

/// Whether `instr` is a region op the forward walk knows, and `scan` enters it.
fn enters<S: ForwardScan>(scan: &S, instr: &MirInstr) -> bool {
    region_op(instr) && scan.enters(instr)
}

/// Walk one region from its entry state, handing every instruction and the
/// state reaching it to `observer` when one is given. A structured region is
/// one block, replayed from `entry`; a lowered block list first reaches its
/// per-block fixpoint. Region ops recurse through [`walk_op`], and a region
/// terminator hands its state to the frame it names.
fn walk_region<S, O>(
    scan: &S,
    entry: Option<S::State>,
    blocks: &[MirBlock],
    mut observer: Option<&mut O>,
    frames: &mut Vec<Frame<S::State>>,
) -> Result<Flow<S::State>, OwnershipError>
where
    S: ForwardScan,
    O: ForwardObserver<S::State>,
{
    let Some(entry) = entry else {
        return Ok(Flow::unreachable());
    };
    if blocks.is_empty() {
        return Ok(Flow::falls_off(entry));
    }
    let in_states = match blocks {
        [block] if terminator_targets(&block.term).is_empty() => vec![Some(entry)],
        _ => region_in_states(scan, &entry, blocks, frames),
    };
    let mut flow = Flow::unreachable();
    for (b, block) in blocks.iter().enumerate() {
        let Some(state) = in_states[b].clone() else {
            continue;
        };
        let Some(state) = walk_block(
            scan,
            state,
            block,
            &mut flow,
            observer.as_deref_mut(),
            frames,
        )?
        else {
            continue;
        };
        match block.term {
            MirTerm::TryYield
            | MirTerm::Yield(_)
            | MirTerm::ComptimeYield(_)
            | MirTerm::ElifCond(_) => add_state(scan, &mut flow.normal, Some(&state)),
            MirTerm::Return(_) | MirTerm::ReturnWithCleanup { .. } => {
                add_state(scan, &mut flow.exits, Some(&state));
            }
            MirTerm::EscapeJump { target, .. } => flow.escapes.push((target, state)),
            MirTerm::Break { .. }
            | MirTerm::Continue { .. }
            | MirTerm::ComptimeBreak
            | MirTerm::ComptimeContinue
            | MirTerm::MatchComplete
            | MirTerm::MatchNext => {
                if let Some((target, resume)) = region_exit(&block.term) {
                    leave_to(scan, frames, target, resume, &state);
                }
            }
            // `hlcf.unreachable` checks nothing: no value reaches past it.
            MirTerm::Jump(_)
            | MirTerm::Branch { .. }
            | MirTerm::ComptimeBranch { .. }
            | MirTerm::ComptimeFor { .. }
            | MirTerm::Unreachable => {}
        }
    }
    Ok(flow)
}

/// Walk one block's instructions from `state`, collecting their raise
/// states and their nested ops' abnormal channels into `flow`. `None` when
/// the block's end is unreachable (after a `raise`, or a region op that
/// never falls off).
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
    for (i, instr) in block.instrs.iter().enumerate() {
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
        if instr.is_raise_point() {
            let seed = scan.raise_seed(&state, block.instrs.get(i + 1));
            add_state(scan, &mut flow.raises, Some(&seed));
        }
        if !matches!(instr, MirInstr::Raise { .. }) {
            current = Some(state);
        }
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
        MirInstr::Loop { label, body } => {
            walk_loop(scan, entry, FrameKind::Loop(*label), body, observer, frames)
        }
        // The else region is `unreachable` once `LowerSemanticCF` has run.
        MirInstr::ComptimeFor { body, .. } => {
            walk_loop(scan, entry, FrameKind::ComptimeFor, body, observer, frames)
        }
        MirInstr::Match { cases, orelse } => {
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
    target: FrameKind,
    resume: bool,
    state: &S::State,
) {
    if let Some(frame) = frames
        .iter_mut()
        .rev()
        .find(|frame| names(target, frame.kind))
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
    kind: FrameKind,
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
    kind: FrameKind,
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
    cases: &[Vec<MirBlock>],
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
            FrameKind::MatchCase,
            case,
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

/// Walk the four regions of one `try`: the body from `entry`, `else` from the
/// body's normal completion, the handler from the body's raise points, and
/// `finally` from every channel (checked once from their join; its own
/// channels compose per entering channel, as the runtime runs it).
///
/// A `try` is transparent to `break`, `continue`, and `match` exits, but one
/// with a `finally` runs it before them, as upstream's `lowerTryFinally`
/// copies it before each terminator leaving the `try`: the states the parts
/// hand the enclosing frames are held back and walked through `finally`
/// before they reach their frames.
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
        body,
        handler,
        orelse,
        finalbody,
        ..
    } = instr
    else {
        return Ok(Flow::falls_off(entry));
    };
    let held: Vec<_> = if finalbody.is_some() {
        frames
            .iter_mut()
            .map(|frame| (frame.leave.take(), frame.resume.take()))
            .collect()
    } else {
        Vec::new()
    };
    let body_flow = walk_region(scan, Some(entry), body, observer.as_deref_mut(), frames)?;
    let mut normal = None;
    let mut raises = None;
    let mut exits = body_flow.exits.clone();
    let mut escapes = body_flow.escapes.clone();
    if let Some(orelse) = orelse {
        let else_flow = walk_region(
            scan,
            body_flow.normal.clone(),
            orelse,
            observer.as_deref_mut(),
            frames,
        )?;
        add_state(scan, &mut normal, else_flow.normal.as_ref());
        add_state(scan, &mut raises, else_flow.raises.as_ref());
        add_state(scan, &mut exits, else_flow.exits.as_ref());
        escapes.extend(else_flow.escapes);
    } else {
        add_state(scan, &mut normal, body_flow.normal.as_ref());
    }
    if let Some((_, handler)) = handler {
        let handler_flow = walk_region(
            scan,
            body_flow.raises.clone(),
            handler,
            observer.as_deref_mut(),
            frames,
        )?;
        add_state(scan, &mut normal, handler_flow.normal.as_ref());
        add_state(scan, &mut raises, handler_flow.raises.as_ref());
        add_state(scan, &mut exits, handler_flow.exits.as_ref());
        escapes.extend(handler_flow.escapes);
    } else {
        add_state(scan, &mut raises, body_flow.raises.as_ref());
    }
    let Some(finalbody) = finalbody else {
        return Ok(Flow {
            normal,
            raises,
            exits,
            escapes,
        });
    };
    let leaving: Vec<_> = frames
        .iter_mut()
        .zip(held)
        .map(|(frame, (leave, resume))| {
            (
                std::mem::replace(&mut frame.leave, leave),
                std::mem::replace(&mut frame.resume, resume),
            )
        })
        .collect();
    if let Some(observer) = observer {
        let mut all = normal.clone();
        add_state(scan, &mut all, raises.as_ref());
        add_state(scan, &mut all, exits.as_ref());
        for (leave, resume) in &leaving {
            add_state(scan, &mut all, leave.as_ref());
            add_state(scan, &mut all, resume.as_ref());
        }
        for (_, state) in &escapes {
            add_state(scan, &mut all, Some(state));
        }
        let mut scratch: Vec<Frame<S::State>> =
            frames.iter().map(|frame| Frame::new(frame.kind)).collect();
        walk_region(scan, all, finalbody, Some(observer), &mut scratch)?;
    }
    let normal_final = walk_region(scan, normal, finalbody, None::<&mut O>, frames)?;
    let raising_final = walk_region(scan, raises, finalbody, None::<&mut O>, frames)?;
    let exiting_final = walk_region(scan, exits, finalbody, None::<&mut O>, frames)?;
    let mut raises = raising_final.normal;
    add_state(scan, &mut raises, normal_final.raises.as_ref());
    add_state(scan, &mut raises, raising_final.raises.as_ref());
    add_state(scan, &mut raises, exiting_final.raises.as_ref());
    let mut exits = exiting_final.normal;
    add_state(scan, &mut exits, normal_final.exits.as_ref());
    add_state(scan, &mut exits, raising_final.exits.as_ref());
    add_state(scan, &mut exits, exiting_final.exits.as_ref());
    let mut through = Flow {
        normal: normal_final.normal,
        raises,
        exits,
        escapes: [
            normal_final.escapes,
            raising_final.escapes,
            exiting_final.escapes,
        ]
        .concat(),
    };
    for (target, state) in escapes {
        let finished = walk_region(scan, Some(state), finalbody, None::<&mut O>, frames)?;
        through.absorb_abnormal(scan, &finished);
        through
            .escapes
            .extend(finished.normal.map(|state| (target, state)));
    }
    for (index, (leave, resume)) in leaving.into_iter().enumerate() {
        for (state, resumes) in [(leave, false), (resume, true)] {
            let finished = walk_region(scan, state, finalbody, None::<&mut O>, frames)?;
            through.absorb_abnormal(scan, &finished);
            let frame = &mut frames[index];
            let set = if resumes {
                &mut frame.resume
            } else {
                &mut frame.leave
            };
            add_state(scan, set, finished.normal.as_ref());
        }
    }
    Ok(through)
}

/// Per-block in-states of a lowered block list at the dataflow fixpoint:
/// `in[b] = ⨆ out[pred]`, `out[b] = transfer(in[b])`. The entry state is a
/// real program state, not the lattice bottom, so unvisited blocks stay
/// absent (a loop header must not join a definite preheader move with a
/// seeded backedge and report it forever). A block an escape from a `try`
/// part enters also joins the escape's state. The region terminators of the
/// fixpoint's walks fill frames of their own, which the replay refills.
fn region_in_states<S: ForwardScan>(
    scan: &S,
    entry: &S::State,
    blocks: &[MirBlock],
    frames: &[Frame<S::State>],
) -> Vec<Option<S::State>> {
    let nb = blocks.len();
    let preds = block_predecessors(blocks);
    let mut in_states: Vec<Option<S::State>> = vec![None; nb];
    let mut out_states: Vec<Option<S::State>> = vec![None; nb];
    let mut escape_out: Vec<Vec<(MirBlockId, S::State)>> = vec![Vec::new(); nb];
    let mut escape_sources: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); nb];
    let mut scratch: Vec<Frame<S::State>> =
        frames.iter().map(|frame| Frame::new(frame.kind)).collect();
    let mut worklist = Worklist::forward(blocks);
    while let Some(b) = worklist.pop() {
        let new_in = if b == 0 || (preds[b].is_empty() && escape_sources[b].is_empty()) {
            entry.clone()
        } else {
            let escaped = escape_sources[b].iter().flat_map(|source| {
                escape_out[*source]
                    .iter()
                    .filter(|(target, _)| *target == b)
                    .map(|(_, state)| state)
            });
            let mut predecessors = preds[b]
                .iter()
                .filter_map(|predecessor| out_states[*predecessor].as_ref())
                .chain(escaped);
            let Some(first) = predecessors.next() else {
                continue;
            };
            predecessors.fold(first.clone(), |acc, predecessor| {
                scan.join(&acc, predecessor)
            })
        };
        if in_states[b].as_ref() == Some(&new_in) {
            continue;
        }
        let mut escapes = Vec::new();
        let new_out = transfer_block(
            scan,
            new_in.clone(),
            &blocks[b].instrs,
            &mut scratch,
            &mut escapes,
        );
        in_states[b] = Some(new_in);
        if escape_out[b] != escapes {
            for (target, _) in escape_out[b].iter().chain(&escapes) {
                worklist.push(*target);
            }
            for (target, _) in &escapes {
                if let Some(sources) = escape_sources.get_mut(*target) {
                    sources.insert(b);
                }
            }
            escape_out[b] = escapes;
        }
        if out_states[b] != new_out {
            out_states[b] = new_out;
            worklist.push_successors(&blocks[b].term);
        }
    }
    in_states
}

/// Apply a block's instructions to a state without reporting; a nested
/// region op continues from its normal completion, its escapes collected in
/// `escapes`, and an always-raising or always-exiting one, like a `raise`,
/// leaves the rest of the block unreachable.
fn transfer_block<S: ForwardScan>(
    scan: &S,
    mut state: S::State,
    instrs: &[MirInstr],
    frames: &mut Vec<Frame<S::State>>,
    escapes: &mut Vec<(MirBlockId, S::State)>,
) -> Option<S::State> {
    for instr in instrs {
        if enters(scan, instr) {
            let nested = walk_op(
                scan,
                state,
                instr,
                None::<&mut NoObserver<S::State>>,
                frames,
            )
            .ok()?;
            escapes.extend(nested.escapes);
            state = nested.normal?;
        } else {
            scan.transfer(&mut state, instr);
            if matches!(instr, MirInstr::Raise { .. }) {
                return None;
            }
        }
    }
    Some(state)
}

/// A frame of the backward walk: the states live where its two exits go.
struct BackFrame<T> {
    kind: FrameKind,
    leave: T,
    resume: T,
}

/// One backward walk: the frames enclosing the region being walked, and
/// the channels a raise and a `return` there leave through.
struct BackWalk<'s, S: BackwardScan> {
    scan: &'s S,
    frames: Vec<BackFrame<S::State>>,
    /// What a raising instruction keeps live: the live-in of the handler
    /// (or the `finally`) a raise there reaches. `None` outside every `try`.
    raise: Option<S::State>,
    /// What a `return` leaves live: the live-in of the `finally`s it
    /// crosses on the return path.
    exit: S::State,
}

impl<S: BackwardScan> BackWalk<'_, S> {
    /// Walk one region backward from `exit`, the state its yields continue
    /// to, returning its live-in. A structured region is one block, walked
    /// once; a lowered block list reaches its per-block fixpoint first and
    /// is then replayed for `recorder`.
    fn region<R: BackwardRecorder<S::State>>(
        &mut self,
        blocks: &[MirBlock],
        exit: &S::State,
        recorder: Option<&mut R>,
    ) -> S::State {
        match blocks {
            [] => return exit.clone(),
            [block] if terminator_targets(&block.term).is_empty() => {
                return self.block(block, exit, &[], recorder);
            }
            _ => {}
        }
        let preds = block_predecessors(blocks);
        let mut live_in = vec![S::State::default(); blocks.len()];
        let mut worklist = Worklist::backward(blocks);
        while let Some(b) = worklist.pop() {
            let incoming = self.block(&blocks[b], exit, &live_in, None::<&mut R>);
            if incoming != live_in[b] {
                live_in[b] = incoming;
                for predecessor in &preds[b] {
                    worklist.push(*predecessor);
                }
            }
        }
        if let Some(recorder) = recorder {
            for block in blocks {
                self.block(block, exit, &live_in, Some(&mut *recorder));
            }
        }
        live_in.swap_remove(0)
    }

    /// Walk one block from its terminator back, returning its live-in;
    /// `live_in` holds its lowered successors' live-ins.
    fn block<R: BackwardRecorder<S::State>>(
        &mut self,
        block: &MirBlock,
        exit: &S::State,
        live_in: &[S::State],
        mut recorder: Option<&mut R>,
    ) -> S::State {
        let out = self.live_out(&block.term, exit, live_in);
        let mut live = out.clone();
        self.scan.terminate(&mut live, block);
        for instr in block.instrs.iter().rev() {
            if region_op(instr) && self.scan.enters(instr) {
                live = self.op(instr, &live, recorder.as_deref_mut());
            } else {
                self.scan.transfer(&mut live, instr);
                if let Some(raise) = self.raise.as_ref().filter(|_| raises_to_handler(instr)) {
                    self.scan.join(&mut live, raise);
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
                    raise: self.raise.as_ref(),
                },
            );
        }
        live
    }

    /// The state live after a block's terminator: `exit` after a yield,
    /// the return channel after a `return`, the named frame's set after a
    /// region terminator, and the join of its successors' live-ins after a
    /// lowered branch.
    fn live_out(&self, term: &MirTerm, exit: &S::State, live_in: &[S::State]) -> S::State {
        match term {
            MirTerm::TryYield
            | MirTerm::Yield(_)
            | MirTerm::ComptimeYield(_)
            | MirTerm::ElifCond(_) => exit.clone(),
            MirTerm::Return(_) | MirTerm::ReturnWithCleanup { .. } => self.exit.clone(),
            MirTerm::EscapeJump { .. } | MirTerm::Unreachable => S::State::default(),
            MirTerm::Break { .. }
            | MirTerm::Continue { .. }
            | MirTerm::ComptimeBreak
            | MirTerm::ComptimeContinue
            | MirTerm::MatchComplete
            | MirTerm::MatchNext => {
                let Some((target, resume)) = region_exit(term) else {
                    return S::State::default();
                };
                self.frames
                    .iter()
                    .rev()
                    .find(|frame| names(target, frame.kind))
                    .map(|frame| {
                        if resume {
                            frame.resume.clone()
                        } else {
                            frame.leave.clone()
                        }
                    })
                    .unwrap_or_default()
            }
            MirTerm::Jump(_)
            | MirTerm::Branch { .. }
            | MirTerm::ComptimeBranch { .. }
            | MirTerm::ComptimeFor { .. } => {
                let mut out = S::State::default();
                for successor in terminator_targets(term) {
                    if let Some(live) = live_in.get(successor) {
                        self.scan.join(&mut out, live);
                    }
                }
                out
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
            MirInstr::Loop { label, body } => {
                self.looped(FrameKind::Loop(*label), body, exit, recorder)
            }
            MirInstr::ComptimeFor { body, .. } => {
                self.looped(FrameKind::ComptimeFor, body, exit, recorder)
            }
            MirInstr::Match { cases, orelse } => {
                let mut next = self.region(orelse, exit, recorder.as_deref_mut());
                for case in cases.iter().rev() {
                    self.frames.push(BackFrame {
                        kind: FrameKind::MatchCase,
                        leave: exit.clone(),
                        resume: next,
                    });
                    next = self.region(case, &S::State::default(), recorder.as_deref_mut());
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
        kind: FrameKind,
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
        kind: FrameKind,
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
    /// walks it with Mojito's `finally` kept: the `finally` from `exit`
    /// under the enclosing channels, then the handler and `else` from the
    /// `finally`'s live-in, a raise in them reaching the `finally` and the
    /// enclosing raise's observers, then the body from the `else`'s live-in
    /// (or the `finally`'s), a raise in it reaching the handler (or what a
    /// raise from the handler would). Inside the parts, a `return` or an
    /// exit to an enclosing frame leaves live the `finally`'s live-in when
    /// walked from where the exit goes, as `lowerTryFinally` copies the
    /// `finally` before each exiting terminator. Only the `finally`'s
    /// normal-path walk is recorded.
    fn tried<R: BackwardRecorder<S::State>>(
        &mut self,
        instr: &MirInstr,
        exit: &S::State,
        mut recorder: Option<&mut R>,
    ) -> S::State {
        let MirInstr::Try {
            body,
            handler,
            orelse,
            finalbody,
            ..
        } = instr
        else {
            return exit.clone();
        };
        let mut fin = exit.clone();
        let mut returns = self.exit.clone();
        let mut enclosing_frames = Vec::new();
        if let Some(fb) = finalbody {
            fin = self.through(fb, exit, recorder.as_deref_mut());
            returns = self.through(fb, &returns, None::<&mut R>);
            for index in 0..self.frames.len() {
                let leave = self.frames[index].leave.clone();
                let resume = self.frames[index].resume.clone();
                let through_leave = self.through(fb, &leave, None::<&mut R>);
                let through_resume = self.through(fb, &resume, None::<&mut R>);
                self.frames[index].leave = through_leave;
                self.frames[index].resume = through_resume;
                enclosing_frames.push((leave, resume));
            }
        }
        let mut outward = fin.clone();
        if let Some(raise) = &self.raise {
            self.scan.join(&mut outward, raise);
        }
        let enclosing_raise = self.raise.replace(outward);
        let enclosing_exit = std::mem::replace(&mut self.exit, returns.clone());
        let mut handler_in = None;
        if let Some((_, h)) = handler {
            let mut live = self.region(h, &fin, recorder.as_deref_mut());
            self.scan.enter(&mut live, h);
            handler_in = Some(live);
        }
        let mut else_in = None;
        if let Some(e) = orelse {
            let mut live = self.region(e, &fin, recorder.as_deref_mut());
            self.scan.enter(&mut live, e);
            else_in = Some(live);
        }
        if let (Some((binding, _)), Some(live)) = (handler, &handler_in) {
            let mut raise = live.clone();
            if let Some(binding) = binding {
                self.scan.define(&mut raise, *binding);
            }
            self.raise = Some(raise);
        }
        let mut live = self.region(
            body,
            else_in.as_ref().unwrap_or(&fin),
            recorder.as_deref_mut(),
        );
        self.scan.enter(&mut live, body);
        self.raise = enclosing_raise;
        self.exit = enclosing_exit;
        for (frame, (leave, resume)) in self.frames.iter_mut().zip(enclosing_frames) {
            frame.leave = leave;
            frame.resume = resume;
        }
        if let Some(recorder) = recorder {
            recorder.try_op(
                instr,
                &TryChannels {
                    after: exit.clone(),
                    fin,
                    returns,
                    handler: handler_in,
                },
            );
        }
        live
    }

    /// The live-in of a `finally` run on the way to where `exit` is live,
    /// under the enclosing channels, as its `try` sees it.
    fn through<R: BackwardRecorder<S::State>>(
        &mut self,
        finalbody: &[MirBlock],
        exit: &S::State,
        recorder: Option<&mut R>,
    ) -> S::State {
        let mut live = self.region(finalbody, exit, recorder);
        self.scan.enter(&mut live, finalbody);
        live
    }
}

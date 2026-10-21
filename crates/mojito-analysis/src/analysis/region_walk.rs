//! The region walks the ownership checks share, each generic over the state
//! it carries.
//!
//! The forward walk is upstream's `UninitializedValueScan` shape in
//! `CheckLifetimes`: an `if` joins its arms (`checkIfOp`), a loop iterates
//! its body until its continue set is stable (`checkLoopOp`), a `match` case
//! starts from its predecessors' `match.next` states (`checkMatchOp`), and a
//! `try` walks its four parts with the raise-point rule (`checkTryOp`). The
//! backward walk is `DestructorInsertion`'s shape: a `break` reads its loop's
//! break set, a `continue` its continue set, and a loop is dry-run until
//! stable, then replayed once. A lowered block list (MIR text, or a `try`
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
    O: FnMut(&S::State, &MirInstr) -> Result<(), OwnershipError>,
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
}

/// Walk a function body backward from its exits, handing `recorder` each
/// instruction with the state live just before it, exactly once. The walk
/// enters `if`, `comptime if`, loops, `comptime for`, and `match`; a `try` is
/// one instruction to it.
pub(super) fn walk_backward<S, R>(scan: &S, blocks: &[MirBlock], recorder: &mut R) -> S::State
where
    S: BackwardScan,
    R: FnMut(&MirInstr, &S::State),
{
    walk_back_region(
        scan,
        blocks,
        &S::State::default(),
        Some(recorder),
        &mut Vec::new(),
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

/// A frame of the backward walk: the states live where its two exits go.
struct BackFrame<T> {
    kind: FrameKind,
    leave: T,
    resume: T,
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
    matches!(
        instr,
        MirInstr::Try { .. }
            | MirInstr::If { .. }
            | MirInstr::Loop { .. }
            | MirInstr::ComptimeIf { .. }
            | MirInstr::ComptimeFor { .. }
            | MirInstr::Match { .. }
    ) && scan.enters(instr)
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
    O: FnMut(&S::State, &MirInstr) -> Result<(), OwnershipError>,
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
    mut state: S::State,
    block: &MirBlock,
    flow: &mut Flow<S::State>,
    mut observer: Option<&mut O>,
    frames: &mut Vec<Frame<S::State>>,
) -> Result<Option<S::State>, OwnershipError>
where
    S: ForwardScan,
    O: FnMut(&S::State, &MirInstr) -> Result<(), OwnershipError>,
{
    for (i, instr) in block.instrs.iter().enumerate() {
        if enters(scan, instr) {
            let nested = walk_op(scan, state, instr, observer.as_deref_mut(), frames)?;
            flow.absorb_abnormal(scan, &nested);
            let Some(normal) = nested.normal else {
                return Ok(None);
            };
            state = normal;
            continue;
        }
        if let Some(observer) = observer.as_deref_mut() {
            observer(&state, instr)?;
        }
        scan.transfer(&mut state, instr);
        if instr.is_raise_point() {
            let seed = scan.raise_seed(&state, block.instrs.get(i + 1));
            add_state(scan, &mut flow.raises, Some(&seed));
        }
        if matches!(instr, MirInstr::Raise { .. }) {
            return Ok(None);
        }
    }
    Ok(Some(state))
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
    O: FnMut(&S::State, &MirInstr) -> Result<(), OwnershipError>,
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
    O: FnMut(&S::State, &MirInstr) -> Result<(), OwnershipError>,
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
    O: FnMut(&S::State, &MirInstr) -> Result<(), OwnershipError>,
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
    O: FnMut(&S::State, &MirInstr) -> Result<(), OwnershipError>,
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
    O: FnMut(&S::State, &MirInstr) -> Result<(), OwnershipError>,
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
    O: FnMut(&S::State, &MirInstr) -> Result<(), OwnershipError>,
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
    O: FnMut(&S::State, &MirInstr) -> Result<(), OwnershipError>,
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

/// Walk one region backward from `exit`, the state its yields continue to,
/// returning its live-in. A structured region is one block, walked once; a
/// lowered block list reaches its per-block fixpoint first and is then
/// replayed for `recorder`.
fn walk_back_region<S, R>(
    scan: &S,
    blocks: &[MirBlock],
    exit: &S::State,
    recorder: Option<&mut R>,
    frames: &mut Vec<BackFrame<S::State>>,
) -> S::State
where
    S: BackwardScan,
    R: FnMut(&MirInstr, &S::State),
{
    match blocks {
        [] => return exit.clone(),
        [block] if terminator_targets(&block.term).is_empty() => {
            let out = back_live_out(scan, &block.term, exit, frames, &[]);
            return walk_back_block(scan, &block.instrs, out, recorder, frames);
        }
        _ => {}
    }
    let preds = block_predecessors(blocks);
    let mut live_in = vec![S::State::default(); blocks.len()];
    let mut worklist = Worklist::backward(blocks);
    while let Some(b) = worklist.pop() {
        let out = back_live_out(scan, &blocks[b].term, exit, frames, &live_in);
        let incoming = walk_back_block(scan, &blocks[b].instrs, out, None::<&mut R>, frames);
        if incoming != live_in[b] {
            live_in[b] = incoming;
            for predecessor in &preds[b] {
                worklist.push(*predecessor);
            }
        }
    }
    if let Some(recorder) = recorder {
        for block in blocks {
            let out = back_live_out(scan, &block.term, exit, frames, &live_in);
            walk_back_block(scan, &block.instrs, out, Some(&mut *recorder), frames);
        }
    }
    live_in.swap_remove(0)
}

/// The state live after a block's terminator: `exit` after a yield, nothing
/// after a `return`, the named frame's set after a region terminator, and
/// the join of its successors' live-ins after a lowered branch.
fn back_live_out<S: BackwardScan>(
    scan: &S,
    term: &MirTerm,
    exit: &S::State,
    frames: &[BackFrame<S::State>],
    live_in: &[S::State],
) -> S::State {
    match term {
        MirTerm::TryYield
        | MirTerm::Yield(_)
        | MirTerm::ComptimeYield(_)
        | MirTerm::ElifCond(_) => exit.clone(),
        MirTerm::Return(_)
        | MirTerm::ReturnWithCleanup { .. }
        | MirTerm::EscapeJump { .. }
        | MirTerm::Unreachable => S::State::default(),
        MirTerm::Break { .. }
        | MirTerm::Continue { .. }
        | MirTerm::ComptimeBreak
        | MirTerm::ComptimeContinue
        | MirTerm::MatchComplete
        | MirTerm::MatchNext => region_exit(term)
            .and_then(|(target, resume)| {
                frames
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
            })
            .unwrap_or_default(),
        MirTerm::Jump(_)
        | MirTerm::Branch { .. }
        | MirTerm::ComptimeBranch { .. }
        | MirTerm::ComptimeFor { .. } => {
            let mut out = S::State::default();
            for successor in terminator_targets(term) {
                if let Some(live) = live_in.get(successor) {
                    scan.join(&mut out, live);
                }
            }
            out
        }
    }
}

/// Walk one block's instructions backward from `live`, its live-out.
fn walk_back_block<S, R>(
    scan: &S,
    instrs: &[MirInstr],
    mut live: S::State,
    mut recorder: Option<&mut R>,
    frames: &mut Vec<BackFrame<S::State>>,
) -> S::State
where
    S: BackwardScan,
    R: FnMut(&MirInstr, &S::State),
{
    for instr in instrs.iter().rev() {
        live = match instr {
            MirInstr::If { .. }
            | MirInstr::ComptimeIf { .. }
            | MirInstr::Loop { .. }
            | MirInstr::ComptimeFor { .. }
            | MirInstr::Match { .. } => {
                walk_back_op(scan, instr, &live, recorder.as_deref_mut(), frames)
            }
            _ => {
                scan.transfer(&mut live, instr);
                if let Some(recorder) = recorder.as_deref_mut() {
                    recorder(instr, &live);
                }
                live
            }
        };
    }
    live
}

/// The live-in of one region op whose live-out is `exit`.
///
/// - An `if`: the else arm from `exit`, then each elif in reverse, its arm
///   from `exit` and its condition from the join of that arm's live-in and
///   the rest's; the then arm's live-in joins the rest's.
/// - A `comptime if`: the join of every arm's live-in.
/// - A loop or `comptime for`: see [`walk_back_loop`].
/// - A `match`: the else region from `exit`, outside this match's frame,
///   then each case in reverse under a frame whose next set is the
///   following case's live-in and whose complete set is `exit`.
fn walk_back_op<S, R>(
    scan: &S,
    instr: &MirInstr,
    exit: &S::State,
    mut recorder: Option<&mut R>,
    frames: &mut Vec<BackFrame<S::State>>,
) -> S::State
where
    S: BackwardScan,
    R: FnMut(&MirInstr, &S::State),
{
    match instr {
        MirInstr::If {
            then,
            elifs,
            orelse,
            ..
        } => {
            let mut rest = walk_back_region(scan, orelse, exit, recorder.as_deref_mut(), frames);
            for elif in elifs.iter().rev() {
                let arm = walk_back_region(scan, &elif.then, exit, recorder.as_deref_mut(), frames);
                scan.join(&mut rest, &arm);
                rest = walk_back_region(scan, &elif.cond, &rest, recorder.as_deref_mut(), frames);
            }
            let mut live = walk_back_region(scan, then, exit, recorder, frames);
            scan.join(&mut live, &rest);
            live
        }
        MirInstr::ComptimeIf { arms, orelse, .. } => {
            let mut live = S::State::default();
            for arm in arms.iter().chain(std::iter::once(orelse)) {
                let arm = walk_back_region(scan, arm, exit, recorder.as_deref_mut(), frames);
                scan.join(&mut live, &arm);
            }
            live
        }
        MirInstr::Loop { label, body } => {
            walk_back_loop(scan, FrameKind::Loop(*label), body, exit, recorder, frames)
        }
        MirInstr::ComptimeFor { body, .. } => {
            walk_back_loop(scan, FrameKind::ComptimeFor, body, exit, recorder, frames)
        }
        MirInstr::Match { cases, orelse } => {
            let mut next = walk_back_region(scan, orelse, exit, recorder.as_deref_mut(), frames);
            for case in cases.iter().rev() {
                frames.push(BackFrame {
                    kind: FrameKind::MatchCase,
                    leave: exit.clone(),
                    resume: next,
                });
                next = walk_back_region(
                    scan,
                    case,
                    &S::State::default(),
                    recorder.as_deref_mut(),
                    frames,
                );
                frames.pop();
            }
            next
        }
        _ => {
            let mut live = exit.clone();
            scan.transfer(&mut live, instr);
            live
        }
    }
}

/// The live-in of a loop (or a `comptime for`) whose live-out is `exit`, as
/// `checkLoopOp` walks it: the body is dry-run under a frame whose break set
/// is `exit` and whose continue set starts empty, until the continue set
/// (the body's live-in) is stable, then replayed once for `recorder`.
fn walk_back_loop<S, R>(
    scan: &S,
    kind: FrameKind,
    body: &[MirBlock],
    exit: &S::State,
    recorder: Option<&mut R>,
    frames: &mut Vec<BackFrame<S::State>>,
) -> S::State
where
    S: BackwardScan,
    R: FnMut(&MirInstr, &S::State),
{
    let framed =
        |resume: S::State, recorder: Option<&mut R>, frames: &mut Vec<BackFrame<S::State>>| {
            frames.push(BackFrame {
                kind,
                leave: exit.clone(),
                resume,
            });
            let live = walk_back_region(scan, body, &S::State::default(), recorder, frames);
            frames.pop();
            live
        };
    let mut resume = S::State::default();
    loop {
        let mut next = framed(resume.clone(), None, frames);
        scan.join(&mut next, &resume);
        if next == resume {
            break;
        }
        resume = next;
    }
    if recorder.is_some() {
        framed(resume.clone(), recorder, frames);
    }
    resume
}

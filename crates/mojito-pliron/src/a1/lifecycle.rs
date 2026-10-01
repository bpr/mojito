//! The cleanup contract: the imported lifecycle schedule as data.
//!
//! It is derived once from verified input and rechecked against the
//! executable operations. This validates an explicit plan; it selects no
//! destructor timing and rediscovers no ownership rule.

use std::collections::HashMap;

use pliron::basic_block::BasicBlock;
use pliron::context::{Context, Ptr};
use pliron::linked_list::ContainsLinkedList;
use pliron::operation::Operation;

use super::attrs::{
    ClosureAttr, ContractAttr, CoreCaptureMode, CoreEvent, CoreLifecycle, CoreStorage, EventKind,
    IdentityAttr, LifecycleAttr, SlotAttr, StoreAttr, UseModeAttr,
};
use super::inventory::{CoreOpKind, EffectClass};
use super::ops::{
    KEY_CLOSURE, KEY_CONTRACT, KEY_IDENTITY, KEY_LIFECYCLE, KEY_SLOT, KEY_STORE, KEY_USE_MODE,
};
use super::outcomes::function_blocks;
use super::verify::{attr, describe, root_slot, variable_slot};
use super::{A1Error, A1ErrorKind};

/// An owner holds no value yet.
pub const UNINITIALIZED: u8 = 1;
/// An owner holds a live value.
pub const LIVE: u8 = 2;
/// An owner's value was moved out.
pub const MOVED: u8 = 4;
/// An owner's value was destroyed or consumed.
pub const ENDED: u8 = 8;

/// The contract the function's current operations imply.
pub fn derive_contract(ctx: &Context, func: Ptr<Operation>) -> Result<ContractAttr, A1Error> {
    let blocks = function_blocks(ctx, func);
    let index: HashMap<Ptr<BasicBlock>, usize> = blocks
        .iter()
        .enumerate()
        .map(|(position, block)| (*block, position))
        .collect();
    let Some(entry) = blocks.first() else {
        return Err(unverified(ctx, func, "a function without an entry block"));
    };
    let mut initial = HashMap::new();
    for op in entry.deref(ctx).iter(ctx) {
        if let Some(slot) = attr::<SlotAttr>(ctx, op, &KEY_SLOT)
            && slot.storage == CoreStorage::Variable
        {
            let state = if slot.param.is_some() {
                LIVE
            } else {
                UNINITIALIZED
            };
            initial.insert(slot.id, state);
        }
    }
    let mut entering: Vec<Option<HashMap<u32, u8>>> = vec![None; blocks.len()];
    entering[0] = Some(initial);
    let mut pending = vec![0usize];
    while let Some(position) = pending.pop() {
        let Some(mut state) = entering[position].clone() else {
            continue;
        };
        for op in blocks[position].deref(ctx).iter(ctx) {
            for (_, owner, after) in transition(ctx, op) {
                state.insert(owner, after);
            }
        }
        let Some(term) = blocks[position].deref(ctx).get_tail() else {
            return Err(unverified(ctx, func, "a block without a terminator"));
        };
        for successor in term.deref(ctx).successors() {
            let Some(target) = index.get(&successor).copied() else {
                return Err(unverified(ctx, term, "a branch out of its function"));
            };
            let merged = match &entering[target] {
                None => state.clone(),
                Some(known) => known
                    .iter()
                    .map(|(owner, bits)| (*owner, bits | state.get(owner).copied().unwrap_or(0)))
                    .collect(),
            };
            if entering[target].as_ref() != Some(&merged) {
                entering[target] = Some(merged);
                pending.push(target);
            }
        }
    }
    let mut events = Vec::new();
    for (position, block) in blocks.iter().enumerate() {
        let mut state = entering[position].clone().unwrap_or_default();
        let mut effectful = 0u64;
        for op in block.deref(ctx).iter(ctx) {
            let chained = CoreOpKind::of(ctx, op).is_some_and(|kind| {
                matches!(
                    kind.effect(),
                    EffectClass::Effectful | EffectClass::Terminator
                )
            });
            for (kind, owner, after) in transition(ctx, op) {
                let identity: IdentityAttr = attr(ctx, op, &KEY_IDENTITY)
                    .ok_or_else(|| unverified(ctx, op, "an operation without identity"))?;
                events.push(CoreEvent {
                    key: identity.local_key().into(),
                    kind,
                    owner,
                    block: position as u64,
                    position: effectful,
                    before: state.get(&owner).copied().unwrap_or(0),
                });
                state.insert(owner, after);
            }
            effectful += u64::from(chained);
        }
    }
    Ok(ContractAttr(events))
}

/// Check the function's operations against the contract it carries.
pub fn check_contract(ctx: &Context, func: Ptr<Operation>) -> Result<(), A1Error> {
    let recorded: ContractAttr = attr(ctx, func, &KEY_CONTRACT)
        .ok_or_else(|| unverified(ctx, func, "a normalized function carries its contract"))?;
    let derived = derive_contract(ctx, func)?;
    let function = attr::<IdentityAttr>(ctx, func, &KEY_IDENTITY)
        .map(|identity| identity.function.0)
        .unwrap_or_default();
    let difference = recorded
        .0
        .iter()
        .zip(&derived.0)
        .find(|(recorded, derived)| recorded != derived);
    if let Some((recorded, derived)) = difference {
        return Err(unverified(
            ctx,
            func,
            &format!(
                "lifecycle event `{function}{}` is contracted as {:?} of owner {} at block {} position {} in states {:#06b}, but the operations hold `{function}{}` as {:?} of owner {} at block {} position {} in states {:#06b}",
                recorded.key,
                recorded.kind,
                recorded.owner,
                recorded.block,
                recorded.position,
                recorded.before,
                derived.key,
                derived.kind,
                derived.owner,
                derived.block,
                derived.position,
                derived.before,
            ),
        ));
    }
    match recorded.0.len().cmp(&derived.0.len()) {
        std::cmp::Ordering::Equal => Ok(()),
        std::cmp::Ordering::Greater => Err(unverified(
            ctx,
            func,
            &format!(
                "lifecycle event `{function}{}` of the contract has no operation",
                recorded.0[derived.0.len()].key
            ),
        )),
        std::cmp::Ordering::Less => Err(unverified(
            ctx,
            func,
            &format!(
                "lifecycle event `{function}{}` is outside the contract",
                derived.0[recorded.0.len()].key
            ),
        )),
    }
}

/// The events `op` is, each with the owner it concerns and the owner's
/// state after. A closure moving several captures is several events.
fn transition(ctx: &Context, op: Ptr<Operation>) -> Vec<(EventKind, u32, u8)> {
    let Some(kind) = CoreOpKind::of(ctx, op) else {
        return Vec::new();
    };
    if kind == CoreOpKind::ClosureMake {
        let Some(closure) = attr::<ClosureAttr>(ctx, op, &KEY_CLOSURE) else {
            return Vec::new();
        };
        return op
            .deref(ctx)
            .operands()
            .zip(&closure.modes)
            .filter(|(place, mode)| **mode == CoreCaptureMode::Move && variable_slot(ctx, *place))
            .filter_map(|(place, _)| root_slot(ctx, place))
            .map(|slot| (EventKind::Move, slot.id, MOVED))
            .collect();
    }
    transition_of_first_operand(ctx, op, kind)
        .into_iter()
        .collect()
}

/// The one event an operation over its first operand's owner is.
fn transition_of_first_operand(
    ctx: &Context,
    op: Ptr<Operation>,
    kind: CoreOpKind,
) -> Option<(EventKind, u32, u8)> {
    let place = op.deref(ctx).operands().next()?;
    let owner = || {
        root_slot(ctx, place)
            .filter(|slot| slot.storage == CoreStorage::Variable)
            .map(|slot| slot.id)
    };
    match kind {
        CoreOpKind::Store => {
            let store: StoreAttr = attr(ctx, op, &KEY_STORE)?;
            matches!(store, StoreAttr::DefVar { .. } | StoreAttr::Caught)
                .then(owner)
                .flatten()
                .map(|owner| (EventKind::Init, owner, LIVE))
        }
        CoreOpKind::Use => {
            let mode: UseModeAttr = attr(ctx, op, &KEY_USE_MODE)?;
            (mode == UseModeAttr::Move)
                .then(owner)
                .flatten()
                .map(|owner| (EventKind::Move, owner, MOVED))
        }
        CoreOpKind::Drop | CoreOpKind::Consume => {
            let lifecycle: LifecycleAttr = attr(ctx, op, &KEY_LIFECYCLE)?;
            let event = match lifecycle.kind {
                CoreLifecycle::DropVar => EventKind::Drop,
                CoreLifecycle::ConsumeVar => EventKind::Consume,
                CoreLifecycle::DropPlace | CoreLifecycle::DropReg | CoreLifecycle::ConsumePlace => {
                    return None;
                }
            };
            owner().map(|owner| (event, owner, ENDED))
        }
        CoreOpKind::Move => variable_slot(ctx, place)
            .then(owner)
            .flatten()
            .map(|owner| (EventKind::Move, owner, MOVED)),
        CoreOpKind::IterInit => {
            let iterator = op.deref(ctx).operands().nth(1)?;
            root_slot(ctx, iterator).map(|slot| (EventKind::Init, slot.id, LIVE))
        }
        _ => None,
    }
}

fn unverified(ctx: &Context, op: Ptr<Operation>, message: &str) -> A1Error {
    A1Error::new(
        A1ErrorKind::Verification,
        format!("{message} ({})", describe(ctx, op)),
    )
}

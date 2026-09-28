//! Dead scalar elimination over the explicit allowlist of pure, total
//! operations. Loads, references, division, calls, and every lifecycle
//! operation stay, used or not.

use pliron::context::{Context, Ptr};
use pliron::operation::Operation;
use pliron::r#type::Typed;

use super::attrs::{CoreRole, CoreStorage, IdentityAttr, InfixAttr, ResolvedAttr, SlotAttr};
use super::inventory::{CoreOpKind, EffectClass};
use super::ops::{KEY_IDENTITY, KEY_INFIX, KEY_RESOLVED, KEY_SLOT};
use super::verify::{attr, describe, is_machine_integer, walk};
use super::{A1Error, A1ErrorKind};

/// What one run of the pass removed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DceReport {
    /// The identity of each removed operation, in removal order.
    pub removed: Vec<String>,
    /// Sweeps over the module, the last of which removed nothing.
    pub sweeps: usize,
}

/// Remove every unused operation of the allowlist, and the register
/// transport that only a removed producer fed, to a fixpoint.
pub fn eliminate_dead_scalars(
    ctx: &mut Context,
    module: Ptr<Operation>,
) -> Result<DceReport, A1Error> {
    let mut report = DceReport::default();
    loop {
        report.sweeps += 1;
        let before = report.removed.len();
        for op in walk(ctx, module) {
            if dead_transport_slot(ctx, op) {
                for store in slot_stores(ctx, op) {
                    report.removed.push(key(ctx, store)?);
                    unthread(ctx, store)?;
                }
            }
            if removable(ctx, op) && !op.deref(ctx).has_use() {
                report.removed.push(key(ctx, op)?);
                Operation::erase(op, ctx);
            }
        }
        if report.removed.len() == before {
            return Ok(report);
        }
    }
}

/// Whether `op` is pure and total: an exact constant, or a wrapping
/// addition of machine integers no overload resolved.
pub fn removable(ctx: &Context, op: Ptr<Operation>) -> bool {
    let Some(kind) = CoreOpKind::of(ctx, op) else {
        return false;
    };
    if kind.effect() != EffectClass::Pure && kind != CoreOpKind::Slot {
        return false;
    }
    let operation = op.deref(ctx);
    match kind {
        CoreOpKind::Const => true,
        CoreOpKind::Binary => {
            let machine = operation
                .operands()
                .chain(operation.results())
                .all(|value| is_machine_integer(ctx, value.get_type(ctx)));
            let unresolved =
                attr::<ResolvedAttr>(ctx, op, &KEY_RESOLVED) == Some(ResolvedAttr(None));
            machine && unresolved && attr::<InfixAttr>(ctx, op, &KEY_INFIX) == Some(InfixAttr::Add)
        }
        CoreOpKind::Slot => attr::<SlotAttr>(ctx, op, &KEY_SLOT)
            .is_some_and(|slot| slot.storage == CoreStorage::Register),
        _ => false,
    }
}

/// Whether `op` is a register slot nothing loads.
fn dead_transport_slot(ctx: &Context, op: Ptr<Operation>) -> bool {
    let register = attr::<SlotAttr>(ctx, op, &KEY_SLOT)
        .is_some_and(|slot| slot.storage == CoreStorage::Register);
    register
        && op.deref(ctx).has_use()
        && op.deref(ctx).uses().all(|usage| {
            matches!(
                role(ctx, usage.user_op()),
                Some(CoreRole::RegisterStore | CoreRole::YieldStore)
            ) && removable(ctx, stored_producer(ctx, usage.user_op()))
        })
}

fn slot_stores(ctx: &Context, slot: Ptr<Operation>) -> Vec<Ptr<Operation>> {
    slot.deref(ctx)
        .uses()
        .map(|usage| usage.user_op())
        .collect()
}

/// The operation whose result a register store transports; the store
/// itself when the value is a block argument.
fn stored_producer(ctx: &Context, store: Ptr<Operation>) -> Ptr<Operation> {
    store
        .deref(ctx)
        .get_operand(1)
        .defining_op()
        .unwrap_or(store)
}

fn role(ctx: &Context, op: Ptr<Operation>) -> Option<CoreRole> {
    attr::<IdentityAttr>(ctx, op, &KEY_IDENTITY).map(|identity| identity.role)
}

fn key(ctx: &Context, op: Ptr<Operation>) -> Result<String, A1Error> {
    attr::<IdentityAttr>(ctx, op, &KEY_IDENTITY)
        .map(|identity| identity.key())
        .ok_or_else(|| {
            A1Error::new(
                A1ErrorKind::Verification,
                format!("an operation without identity ({})", describe(ctx, op)),
            )
        })
}

/// Remove a transport store from its effect chain and erase it.
fn unthread(ctx: &mut Context, store: Ptr<Operation>) -> Result<(), A1Error> {
    let (token_in, token_out) = {
        let operation = store.deref(ctx);
        (operation.operands().last(), operation.results().last())
    };
    let (Some(token_in), Some(token_out)) = (token_in, token_out) else {
        return Err(A1Error::new(
            A1ErrorKind::Verification,
            format!(
                "a store outside the effect chain ({})",
                describe(ctx, store)
            ),
        ));
    };
    token_out.replace_all_uses_with(ctx, &token_in);
    Operation::erase(store, ctx);
    Ok(())
}

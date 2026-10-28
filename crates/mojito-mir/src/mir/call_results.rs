//! Call-result storage: a value its callee returns through memory gets one
//! hidden slot at its call, as upstream's call emission passes a
//! `ByRefResult` argument its destination or a synthesized
//! `__call_result_tmp__`, and each consumer uses that slot by its own
//! convention. See `docs/symbol-map.md`.

use super::calls::owns_droppable_storage;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::{CheckedOp, SemanticAdjustment};
use mojito_types::types::TyKind;

/// A lowered expression's value, as upstream's `CValue` is an rvalue or an
/// lvalue: a register, or the call-result slot
/// ([`crate::mir::CALL_RESULT_SLOT`]) its call wrote it to.
#[derive(Clone, Copy)]
pub(super) enum CValue {
    Value(Reg),
    Slot(VarId),
}

impl<'a> Flatten<'a> {
    /// Lower `e` for a consumer that brings no destination of its own: an
    /// argument, receiver, or operand, a field read's base, a subscript's
    /// base, a condition, or a discarded statement. A value returned through
    /// memory ([`Node::indirect_result`]) lands in its call-result slot; a
    /// conditional expression's lands in the result slot its arms move into.
    pub(super) fn expr_c(&mut self, e: Node<'a>) -> CValue {
        self.expr_c_holding(e, true)
    }

    /// [`Self::expr_c`] for a call operand, which leaves an origin-bearing
    /// view in its register ([`Self::call_result`]).
    pub(super) fn operand_expr_c(&mut self, e: Node<'a>) -> CValue {
        self.expr_c_holding(e, false)
    }

    /// `value`, the lowered `e`, as [`Self::expr_c`] holds it: bound to its
    /// call-result slot right after its producer when `e` returns through
    /// memory, its value is an owned temporary of its own (no reference, no
    /// loan a binding must carry, no materialized borrow source), and it is
    /// not already a slot's forward. Unless `views`, an origin-bearing view
    /// stays in its register, whose provenance keeps the storage it views
    /// live through the call that reads it.
    pub(super) fn call_result(&mut self, e: Node<'a>, value: Reg, views: bool) -> CValue {
        if let Some(&slot) = self.call_result_slots.get(&value.0) {
            return CValue::Slot(slot);
        }
        let forwarded = matches!(
            self.f.blocks[self.cur].instrs.last(),
            Some(MirInstr::UseVar { dest, mode: UseMode::Forward, .. }) if *dest == value
        );
        let ty = self.f.reg_types.get(&value.0).cloned().or_else(|| e.ty());
        let Some(ty) = ty.filter(|ty| {
            (owns_droppable_storage(ty)
                || mojito_types::types::trivially_register_passable_leaf(ty.kind()))
                && (views || mojito_types::types::erase_origin_arguments(ty) == *ty)
        }) else {
            return CValue::Value(value);
        };
        if forwarded
            || !e.indirect_result()
            || mojito_checked::checked::materialized_borrow_owner(e.adjustments()).is_some()
            || !self.aggregate_borrows(e).is_empty()
        {
            return CValue::Value(value);
        }
        CValue::Slot(self.call_result_slot(value, ty, e.raises().is_none()))
    }

    /// Bind `value`, of type `ty`, which its producer returned through
    /// memory, to a call-result slot of its own, right after the producer,
    /// in place when the producer cannot raise (`non_raising`).
    pub(super) fn call_result_slot(&mut self, value: Reg, ty: Ty, non_raising: bool) -> VarId {
        let in_place = self.produced_in_place(value, non_raising);
        let slot = self.var(&format!("{}{}", crate::mir::CALL_RESULT_SLOT, value.0));
        self.var_types.insert(slot, ty.clone());
        self.emit(MirInstr::DefVar {
            var: slot,
            src: value,
            binding_ty: Some(ty),
            in_place,
        });
        self.call_result_slots.insert(value.0, slot);
        slot
    }

    /// `value` read by a consumer that reads it, with the storage it read:
    /// a whole load of a slot, as a named local is read in place.
    pub(super) fn read_c(&mut self, e: Node<'a>, value: CValue) -> (Reg, Option<MirPlace>) {
        let slot = match value {
            CValue::Value(value) => return (value, None),
            CValue::Slot(slot) => slot,
        };
        let ty = self.var_types.get(&slot).cloned();
        let place = MirPlace::root(slot, ty.clone());
        let read = self.fresh_typed(e.location(), Some(slot), ty.unwrap_or_else(Ty::error));
        self.emit(MirInstr::LoadPlace {
            dest: read,
            place: place.clone(),
        });
        (read, Some(place))
    }

    /// `value` taken by a consumer that owns it: the slot's forward, which
    /// a call setup moves to the call.
    pub(super) fn take_c(&mut self, e: Node<'a>, value: CValue) -> Reg {
        let slot = match value {
            CValue::Value(value) => return value,
            CValue::Slot(slot) => slot,
        };
        let ty = self.var_types.get(&slot).cloned().unwrap_or_else(Ty::error);
        let taken = self.fresh_typed(e.location(), Some(slot), ty);
        self.emit(MirInstr::UseVar {
            dest: taken,
            var: slot,
            mode: UseMode::Forward,
            constructor: None,
        });
        taken
    }

    /// `value`, call operand `expression` (an argument, receiver, or
    /// operator operand), used by the convention the checker recorded for
    /// it: one it marked `ReadTemporaryArgument` (or any, when `reads`) is
    /// read, and any other is taken, as an owned operand the checker marked
    /// `ForwardedArgument` is.
    pub(super) fn operand_c(&mut self, expression: Node<'a>, value: CValue, reads: bool) -> Reg {
        let forwarded = expression
            .adjustments()
            .iter()
            .any(|adjustment| matches!(adjustment, SemanticAdjustment::ForwardedArgument));
        let read = expression
            .adjustments()
            .iter()
            .any(|adjustment| matches!(adjustment, SemanticAdjustment::ReadTemporaryArgument));
        if !forwarded && (reads || read) {
            return self.read_c(expression, value).0;
        }
        self.take_c(expression, value)
    }

    /// A condition's `Bool`: its value, or, for a `Boolable` struct the
    /// checker marked `Truthiness`, the `Bool(x)` conversion of it, which
    /// reads a place the checker marked `BorrowReadArgument` where it lies
    /// and a temporary in its call-result slot.
    pub(super) fn condition_value(&mut self, condition: Node<'a>) -> Reg {
        let boolable = condition
            .adjustments()
            .iter()
            .any(|adjustment| matches!(adjustment, SemanticAdjustment::Truthiness));
        if !boolable {
            return self.expr(condition);
        }
        let value = self.read_operand(condition);
        self.bool_conversion(condition.location(), value, condition.ty().as_ref())
    }

    /// `e` read by a consumer with a read convention: a place the checker
    /// marked `BorrowReadArgument` where it lies, and anything else in its
    /// call-result slot, so no copy of `e` runs for the read.
    pub(super) fn read_operand(&mut self, e: Node<'a>) -> Reg {
        let borrowed = e
            .adjustments()
            .iter()
            .any(|adjustment| matches!(adjustment, SemanticAdjustment::BorrowReadArgument));
        borrowed
            .then(|| self.lower_borrowed_read_argument(e))
            .flatten()
            .unwrap_or_else(|| {
                let value = self.expr_c(e);
                self.read_c(e, value).0
            })
    }

    fn expr_c_holding(&mut self, e: Node<'a>, views: bool) -> CValue {
        if let Some(slot) = self.conditional_result_slot(e) {
            return CValue::Slot(slot);
        }
        let value = self.expr(e);
        self.call_result(e, value, views)
    }

    /// The result slot a conditional expression `e` that is not proven
    /// register-passable moves its arm into, when nothing adjusts its
    /// value: the slot is the expression's storage, as upstream moves an
    /// arm into its destination buffer.
    fn conditional_result_slot(&mut self, e: Node<'a>) -> Option<VarId> {
        let CheckedOp::IfExpr {
            cond,
            then_branch,
            else_branch,
        } = e.op()
        else {
            return None;
        };
        let adjusted = e.implicit_conversion().is_some()
            || e.index_normalization().is_some()
            || e.simd_from_length().is_some()
            || e.rebind_target().is_some()
            || e.borrow_mutability().is_some();
        if adjusted || e.register_passable_result() {
            return None;
        }
        let ty = e.ty().filter(|ty| !matches!(ty.kind(), TyKind::Ref(_)))?;
        let slot = self.conditional_into_slot(
            e,
            e.operand_of(*cond)?,
            e.operand_of(*then_branch)?,
            e.operand_of(*else_branch)?,
        );
        self.var_types.entry(slot).or_insert(ty);
        Some(slot)
    }
}

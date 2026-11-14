//! Call-result storage: a value its callee returns through memory gets one
//! hidden slot at its call, as upstream's call emission passes a
//! `ByRefResult` argument its destination or a synthesized
//! `__call_result_tmp__`, and each consumer uses that slot by its own
//! convention. See `docs/symbol-map.md`.

use super::calls::owns_droppable_storage;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::ArgConvention;
use mojito_checked::checked::{
    CheckedCallArgumentSource, CheckedCallContract, CheckedOp, SemanticAdjustment,
};
use mojito_types::types::TyKind;

/// A lowered expression's value, as upstream's `CValue` is an rvalue or an
/// lvalue: a register, or the call-result slot
/// ([`crate::mir::CALL_RESULT_SLOT`]) its call wrote it to.
#[derive(Clone, Copy)]
pub(super) enum CValue {
    Value(Reg),
    Slot(VarId),
}

/// How a call operand binds the parameter its call's selected callee
/// declares for it, as upstream's call emission reads each argument's
/// convention off the callee's signature. An operand no binding names (a
/// `mut`, `ref`, or `out` parameter's, or one no recorded signature covers)
/// is taken.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum OperandBinding {
    /// A `read` parameter or receiver borrows the operand: a temporary is
    /// read in its storage, which the caller destroys once the call
    /// returns.
    Read,
    /// A `var`/`deinit` parameter or receiver, or an owning collector, takes
    /// the operand's storage: a `^` transfer of a whole local hands its slot
    /// over without the move initializer.
    Owned,
}

impl OperandBinding {
    /// The binding of a parameter of effective convention `convention`.
    pub(super) const fn of(convention: Option<ArgConvention>) -> Option<Self> {
        match convention {
            None | Some(ArgConvention::Imm) => Some(Self::Read),
            Some(ArgConvention::Var | ArgConvention::Deinit) => Some(Self::Owned),
            Some(ArgConvention::Mut | ArgConvention::Ref | ArgConvention::Out) => None,
        }
    }

    /// The binding of the run-time argument `source` of the call
    /// `contract` selected ([`CheckedCallContract::arguments`]).
    pub(super) fn of_argument(
        contract: &CheckedCallContract,
        source: CheckedCallArgumentSource,
    ) -> Option<Self> {
        contract
            .arguments
            .iter()
            .find(|argument| argument.source == source)
            .and_then(|argument| Self::of(argument.convention))
    }

    /// The binding of method call `contract`'s receiver: its declared
    /// convention's, a `ref self` taking none whatever its effective
    /// access.
    pub(super) const fn of_receiver(contract: &CheckedCallContract) -> Option<Self> {
        if contract.receiver_requires_place {
            return None;
        }
        Self::of(contract.receiver_convention)
    }
}

impl<'a> Flatten<'a> {
    /// Lower `e` for a consumer that brings no destination of its own: an
    /// argument, receiver, or operand, a field read's base, a subscript's
    /// base, a condition, or a discarded statement. A value returned through
    /// memory ([`Node::indirect_result`]) lands in its call-result slot; a
    /// conditional expression's lands in the result slot its arms move into.
    /// A loan-carrying one holds its loans on that slot
    /// ([`Self::loaned_call_result`]).
    pub(super) fn expr_c(&mut self, e: Node<'a>) -> CValue {
        match self.operand_expr_c(e) {
            CValue::Value(value) => self
                .loaned_call_result(e, value)
                .map_or(CValue::Value(value), CValue::Slot),
            slot @ CValue::Slot(_) => slot,
        }
    }

    /// [`Self::expr_c`] for a call operand, which leaves a loan-carrying
    /// result in its register for an argument anchor to bind first
    /// ([`Self::anchor_temporary_argument`]).
    pub(super) fn operand_expr_c(&mut self, e: Node<'a>) -> CValue {
        if let Some(slot) = self
            .conditional_result_slot(e)
            .or_else(|| self.lambda_storage_variable(e))
        {
            return CValue::Slot(slot);
        }
        let value = self.expr(e);
        self.call_result(e, value)
    }

    /// `value`, the lowered `e`, as [`Self::expr_c`] holds it: bound to its
    /// call-result slot right after its producer when `e` returns through
    /// memory, its value is an owned temporary of its own (no reference, no
    /// loan a binding must carry, no materialized borrow source), and it is
    /// not already a slot's forward.
    pub(super) fn call_result(&mut self, e: Node<'a>, value: Reg) -> CValue {
        self.result_storage(e, value, e.indirect_result())
    }

    /// The source of an implicit conversion whose constructor reads it
    /// (`ConversionReadsSource`), as upstream passes a memory-only `read`
    /// argument by address, with the place the read binds: a named place
    /// read where it lies, so no copy of it runs, and a temporary its
    /// producer returned through memory read in its call-result slot.
    pub(super) fn read_conversion_source(&mut self, e: Node<'a>) -> (Reg, Option<MirPlace>) {
        if let Some((value, place)) = self.read_in_place(e) {
            return (value, Some(place));
        }
        let value = self.expr_unconverted(e);
        if let Some(ty) = e.ty() {
            self.f.reg_types.entry(value.0).or_insert(ty);
        }
        let value = self.result_storage(e, value, e.unconverted_indirect_result());
        self.read_c(e, value)
    }

    /// Named place `e` read where it lies, with its place, as a `read`
    /// parameter binds a memory-only argument's address: `None` for a
    /// temporary or a value with no slot to lend.
    pub(super) fn read_in_place(&mut self, e: Node<'a>) -> Option<(Reg, MirPlace)> {
        if e.is_fresh_temporary() {
            return None;
        }
        let place = self.simple_place(e)?;
        let value = self.lower_borrowed_read_argument(e)?;
        Some((value, place))
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

    /// The storage a borrower of `value`, the lowered `e`, lends: the
    /// call-result slot when its producer returned it through memory
    /// (`indirect`), as upstream's borrowers all sit on the one
    /// `__call_result_tmp__` the call wrote, and otherwise a hidden slot
    /// named `prefix`, upstream's `anonymous` materialization of a
    /// register-passable value.
    pub(super) fn temporary_storage(
        &mut self,
        e: Node<'a>,
        value: Reg,
        indirect: bool,
        prefix: &str,
    ) -> VarId {
        if let Some(&slot) = self.call_result_slots.get(&value.0) {
            return slot;
        }
        let ty = self.f.reg_types.get(&value.0).cloned().or_else(|| e.ty());
        if indirect && let Some(ty) = ty.clone() {
            return self.call_result_slot(value, ty, e.raises().is_none());
        }
        let variable = self.var(&format!("{prefix}{}", value.0));
        if let Some(ty) = ty.clone() {
            self.var_types.insert(variable, ty);
        }
        self.emit(MirInstr::DefVar {
            var: variable,
            src: value,
            binding_ty: ty,
            in_place: false,
        });
        variable
    }

    /// The call-result slot of `value`, the lowered call operand `e`, when
    /// its producer returned it through memory and it carries loans no
    /// argument anchor took ([`Self::anchor_temporary_argument`]): the slot
    /// holds those loans, as upstream's `__call_result_tmp__` is the storage
    /// every borrower of the result sits on, so the consuming call uses the
    /// result where its callee wrote it.
    pub(super) fn loaned_call_result(&mut self, e: Node<'a>, value: Reg) -> Option<VarId> {
        if !e.indirect_result()
            || mojito_checked::checked::materialized_borrow_owner(e.adjustments()).is_some()
            || matches!(
                self.f.body.instrs.last(),
                Some(MirInstr::UseVar { dest, mode: UseMode::Forward, .. }) if *dest == value
            )
        {
            return None;
        }
        let ty = self
            .f
            .reg_types
            .get(&value.0)
            .cloned()
            .or_else(|| e.ty())
            .filter(owns_droppable_storage)?;
        let loans = self.aggregate_borrows(e);
        if loans.is_empty() {
            return None;
        }
        let slot = self.call_result_slot(value, ty, e.raises().is_none());
        self.establish_storage_loans(e, slot, loans);
        Some(slot)
    }

    /// Put `loans`, the borrows the temporary `e` holds, on `variable`, the
    /// storage it was bound to, so the viewed storage outlives every
    /// borrower of that storage.
    pub(super) fn establish_storage_loans(
        &mut self,
        e: Node<'a>,
        variable: VarId,
        loans: Vec<MirLoan>,
    ) {
        let Some(first) = loans.first() else {
            return;
        };
        let marker = self.fresh_typed(e.location(), Some(first.place.root), Ty::none());
        self.aggregate_loans.insert(variable, loans.clone());
        self.emit(MirInstr::EstablishLoans {
            reference: variable,
            loans,
            marker,
            dest_interior: None,
        });
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
    /// operator operand), used by the binding its call recorded for it
    /// ([`Self::operand_reads`]): a read one is read, with the storage it
    /// was read from, the slot whose address a by-address `read` parameter
    /// binds ([`crate::mir::address_call_operands`]); any other is taken,
    /// and has no place.
    pub(super) fn operand_c(
        &mut self,
        expression: Node<'a>,
        value: CValue,
        reads: bool,
    ) -> (Reg, Option<MirPlace>) {
        if self.operand_reads(expression, reads) {
            return self.read_c(expression, value);
        }
        (self.take_c(expression, value), None)
    }

    /// Record how `call`'s positional `args`, the first at position
    /// `first`, and its keyword `kwargs` bind their parameters: by the
    /// effective conventions its selected callee declares
    /// ([`Node::argument_binding`]).
    pub(super) fn bind_call_operands(
        &mut self,
        call: Node<'a>,
        first: usize,
        args: &[Node<'a>],
        kwargs: &[(&str, Node<'a>)],
    ) {
        for (index, &argument) in args.iter().enumerate() {
            let binding =
                call.argument_binding(CheckedCallArgumentSource::Positional(first + index));
            self.bind_operand(argument, binding);
        }
        for (index, &(_, argument)) in kwargs.iter().enumerate() {
            let binding = call.argument_binding(CheckedCallArgumentSource::Keyword(index));
            self.bind_operand(argument, binding);
        }
    }

    /// [`Self::bind_call_operands`] for subscript `subscript`'s arguments
    /// `args`, each run-time index or keyword value its accessor's argument
    /// of that position (a slice occupies one) or keyword, `operand`
    /// resolving a node.
    pub(super) fn bind_subscript_operands(
        &mut self,
        subscript: Node<'a>,
        args: &[mojito_checked::checked::CheckedSubscriptArg],
        operand: impl Fn(mojito_checked::checked::CheckedNodeId) -> Node<'a>,
    ) {
        use mojito_checked::checked::CheckedSubscriptArg;
        let (mut positional, mut keyword) = (0, 0);
        for argument in args {
            let (source, value) = match argument {
                CheckedSubscriptArg::Index(value) => (
                    CheckedCallArgumentSource::Positional(positional),
                    Some(*value),
                ),
                CheckedSubscriptArg::Slice { .. } => {
                    (CheckedCallArgumentSource::Positional(positional), None)
                }
                CheckedSubscriptArg::Keyword { value, .. } => {
                    (CheckedCallArgumentSource::Keyword(keyword), Some(*value))
                }
                CheckedSubscriptArg::KeywordSlice { .. } => {
                    (CheckedCallArgumentSource::Keyword(keyword), None)
                }
            };
            match source {
                CheckedCallArgumentSource::Keyword(_) => keyword += 1,
                _ => positional += 1,
            }
            if let Some(value) = value {
                let binding = subscript.argument_binding(source);
                self.bind_operand(operand(value), binding);
            }
        }
    }

    /// Record `binding` as how operand `operand` binds its parameter.
    pub(super) fn bind_operand(&mut self, operand: Node<'a>, binding: Option<OperandBinding>) {
        match binding {
            Some(binding) => self.operand_bindings.insert(operand.address(), binding),
            None => self.operand_bindings.remove(&operand.address()),
        };
    }

    /// The binding recorded for operand `operand` ([`Self::bind_operand`]).
    pub(super) fn operand_binding(&self, operand: Node<'a>) -> Option<OperandBinding> {
        self.operand_bindings.get(&operand.address()).copied()
    }

    /// Whether call operand `expression` is read rather than taken: it is
    /// not bound owned, and it is a temporary its call binds to a `read`
    /// parameter ([`Node::binds_temporary`]), or the consumer `reads` any
    /// operand.
    pub(super) fn operand_reads(&self, expression: Node<'a>, reads: bool) -> bool {
        match self.operand_binding(expression) {
            Some(OperandBinding::Owned) => false,
            Some(OperandBinding::Read) => reads || expression.binds_temporary(),
            None => reads,
        }
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
            let value = self.expr(condition);
            return self.field_referent(condition, value);
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

    /// `value`, the lowered `e`, as [`Self::call_result`] holds it, its
    /// producer returning it through memory when `indirect`. A compile-time
    /// parameter's value owning droppable storage is materialized in
    /// memory too, as upstream's memory-only parameter constant is.
    fn result_storage(&mut self, e: Node<'a>, value: Reg, indirect: bool) -> CValue {
        if let Some(&slot) = self.call_result_slots.get(&value.0) {
            return CValue::Slot(slot);
        }
        let last = self.f.body.instrs.last();
        let forwarded = matches!(
            last,
            Some(MirInstr::UseVar { dest, mode: UseMode::Forward, .. }) if *dest == value
        );
        let constant = matches!(last, Some(MirInstr::Const { dest, .. }) if *dest == value);
        let ty = self.f.reg_types.get(&value.0).cloned().or_else(|| e.ty());
        let Some(ty) = ty.filter(|ty| {
            owns_droppable_storage(ty)
                || mojito_types::types::trivially_register_passable_leaf(ty.kind())
        }) else {
            return CValue::Value(value);
        };
        let indirect = indirect || constant && owns_droppable_storage(&ty);
        if forwarded
            || !indirect
            || mojito_checked::checked::materialized_borrow_owner(e.adjustments()).is_some()
            || !self.aggregate_borrows(e).is_empty()
        {
            return CValue::Value(value);
        }
        CValue::Slot(self.call_result_slot(value, ty, e.raises().is_none()))
    }

    /// The storage variable of capturing lambda `e`: its storage struct
    /// built in a slot, the
    /// lvalue upstream's `emitClosure` returns, which a reading consumer
    /// lends and an owning one takes.
    fn lambda_storage_variable(&mut self, e: Node<'a>) -> Option<VarId> {
        if !matches!(e.op(), CheckedOp::Lambda { .. }) {
            return None;
        }
        let info = self.nested_info(e)?;
        let storage = info.storage.as_ref()?;
        Some(self.lambda_storage_slot(&info, storage, &e.location()))
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

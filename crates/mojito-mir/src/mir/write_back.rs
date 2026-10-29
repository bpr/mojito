//! Subscript write-back: a value `__getitem__`'s result used as a mutable
//! place is loaded into a temporary, mutated there, and stored back through
//! `__setitem__`, as upstream loads, modifies, and stores a
//! `SubscriptDLValue`. See `docs/symbol-map.md`.

use super::call_results::CValue;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::{
    CheckedCallArgumentSource, CheckedOp, CheckedSubscriptArg, CheckedSubscriptWriteBack,
};

/// One materialized write-back awaiting its setter call: the subscript
/// `target`, its `object`, its accessor `plan`, the temporary `slot` the
/// getter's result lives in, and the receiver and index arguments as
/// evaluated once for both accessors.
pub(super) struct PendingWriteBack<'a> {
    target: Node<'a>,
    object: Node<'a>,
    plan: CheckedSubscriptWriteBack,
    slot: VarId,
    receiver: Reg,
    receiver_place: Option<MirPlace>,
    indices: Vec<WriteBackIndex<'a>>,
}

/// One index argument of a written-back subscript: its `node`, its value
/// before either accessor's adaptation, and the place it was read from when
/// an accessor binds it by reference.
struct WriteBackIndex<'a> {
    node: Node<'a>,
    raw: Reg,
    place: Option<MirPlace>,
}

impl<'a> Flatten<'a> {
    /// The place of subscript `target` when the checker planned its
    /// write-back: the getter's result in a fresh temporary, whose setter
    /// call is pending until the enclosing operation ends
    /// ([`Self::with_write_backs`]). The receiver and the indices are
    /// evaluated once, where written; the getter runs at the enclosing
    /// call's setup when `target` is one of its operands. `None` for any
    /// other node.
    pub(super) fn materialize_write_back_place(&mut self, target: Node<'a>) -> Option<MirPlace> {
        let plan = target.subscript_write_back()?;
        let object = target.object()?;
        let index_nodes = match target.op() {
            CheckedOp::Index { .. } => vec![target.index()?],
            CheckedOp::MultiIndex { args, .. } => args
                .iter()
                .map(|argument| match argument {
                    CheckedSubscriptArg::Index(id) => target.operand_of(*id),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?,
            _ => return None,
        };
        let (receiver, receiver_place) = self.lower_call_receiver(object);
        let indices = index_nodes
            .into_iter()
            .enumerate()
            .map(|(position, node)| {
                let source = CheckedCallArgumentSource::Positional(position);
                let retain = Self::checked_call_source_requires_place(&plan.getter, source)
                    || Self::checked_call_source_requires_place(&plan.setter, source);
                let (raw, place) = self.lower_augmented_argument_source(node, retain);
                WriteBackIndex { node, raw, place }
            })
            .collect::<Vec<_>>();
        self.begin_awaited_setup_read(target);
        let args = indices
            .iter()
            .enumerate()
            .map(|(position, index)| {
                MirSubscriptArg::Index(self.apply_checked_call_value_adjustments(
                    &plan.getter,
                    CheckedCallArgumentSource::Positional(position),
                    index.raw,
                    index.node.location(),
                ))
            })
            .collect::<Vec<_>>();
        let arg_places = indices
            .iter()
            .enumerate()
            .map(|(position, index)| {
                Self::checked_call_source_place(
                    &plan.getter,
                    CheckedCallArgumentSource::Positional(position),
                    &index.place,
                )
            })
            .collect::<Vec<_>>();
        let getter_call =
            self.mir_subscript_call_contract(plan.getter.clone(), &args, target.location());
        self.emit_checked_call_boundary(&plan.getter, &target.location());
        let current = self.fresh_typed(target.location(), None, plan.getter.result_ty.clone());
        self.record_call(current, Some(getter_call.target.clone()), 0);
        let call = Some(Box::new(getter_call));
        let base_place = receiver_place.clone().map(Box::new);
        self.emit(match (target.op(), args.as_slice()) {
            (CheckedOp::Index { .. }, [MirSubscriptArg::Index(index)]) => MirInstr::Index {
                dest: current,
                base: receiver,
                index: *index,
                base_place,
                index_place: arg_places.into_iter().next().flatten().map(Box::new),
                call,
                intrinsic: None,
            },
            _ => MirInstr::MultiIndex {
                dest: current,
                object: receiver,
                args,
                object_place: base_place,
                arg_places,
                kwargs: Vec::new(),
                kwarg_places: Vec::new(),
                call,
            },
        });
        let slot = self.var(&format!("{}{}", crate::mir::CALL_RESULT_SLOT, current.0));
        self.var_types.insert(slot, plan.element.clone());
        self.emit(MirInstr::DefVar {
            var: slot,
            src: current,
            binding_ty: Some(plan.element.clone()),
            in_place: false,
        });
        let place = MirPlace::root(slot, Some(plan.element.clone()));
        self.pending_write_backs.push(PendingWriteBack {
            target,
            object,
            plan,
            slot,
            receiver,
            receiver_place,
            indices,
        });
        Some(place)
    }

    /// Run `lower`, an operation that may use write-back places, then store
    /// each place it materialized back through its setter, innermost first,
    /// as upstream stores a `SubscriptDLValue` once the operation through it
    /// returns.
    pub(super) fn with_write_backs<R>(&mut self, lower: impl FnOnce(&mut Self) -> R) -> R {
        let enclosing = std::mem::take(&mut self.pending_write_backs);
        let lowered = lower(self);
        self.flush_write_backs();
        self.pending_write_backs = enclosing;
        lowered
    }

    /// Emit the setter call of every pending write-back, innermost first.
    pub(super) fn flush_write_backs(&mut self) {
        while let Some(pending) = self.pending_write_backs.pop() {
            self.emit_write_back(pending);
        }
    }

    /// Store `pending`'s temporary back through its setter, with the
    /// receiver and indices as the getter left them.
    fn emit_write_back(&mut self, pending: PendingWriteBack<'a>) {
        let PendingWriteBack {
            target,
            object,
            plan,
            slot,
            receiver,
            receiver_place,
            indices,
        } = pending;
        let setter = &plan.setter;
        let setter_receiver = self.reload_augmented_source(
            receiver,
            &receiver_place,
            matches!(
                plan.getter.receiver_convention,
                Some(mojito_ast::ast::ArgConvention::Mut | mojito_ast::ast::ArgConvention::Ref)
            ),
            object.location(),
        );
        let mut args = Vec::with_capacity(indices.len());
        let mut arg_places = Vec::with_capacity(indices.len());
        for (position, index) in indices.iter().enumerate() {
            let source = CheckedCallArgumentSource::Positional(position);
            let raw = self.reload_augmented_source(
                index.raw,
                &index.place,
                Self::checked_call_source_mutates(&plan.getter, source),
                index.node.location(),
            );
            args.push(MirSubscriptArg::Index(
                self.apply_checked_call_value_adjustments(
                    setter,
                    source,
                    raw,
                    index.node.location(),
                ),
            ));
            arg_places.push(Self::checked_call_source_place(
                setter,
                source,
                &index.place,
            ));
        }
        let (value, value_place) = self.augmented_setter_value(
            target,
            setter,
            CheckedCallArgumentSource::Keyword(0),
            CValue::Slot(slot),
        );
        let setter_call =
            self.mir_subscript_call_contract(setter.clone(), &args, target.location());
        self.emit_checked_call_boundary(setter, &target.location());
        self.emit(MirInstr::MultiSet {
            receiver: setter_receiver,
            receiver_place: receiver_place.map(Box::new),
            args,
            arg_places,
            value,
            value_place: value_place.map(Box::new),
            value_keyword: plan.value_keyword.clone(),
            call: Box::new(setter_call),
        });
    }
}

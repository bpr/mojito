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

/// One index argument of a written-back subscript: its `node`, the call
/// `source` it binds, the keyword `name` it is passed under (`None` for a
/// positional one), its value before either accessor's adaptation, and the
/// place it was read from when an accessor binds it by reference.
struct WriteBackIndex<'a> {
    node: Node<'a>,
    source: CheckedCallArgumentSource,
    name: Option<String>,
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
            CheckedOp::Index { .. } => vec![(target.index()?, None)],
            CheckedOp::MultiIndex { args, .. } => args
                .iter()
                .map(|argument| match argument {
                    CheckedSubscriptArg::Index(id) => target.operand_of(*id).map(|n| (n, None)),
                    CheckedSubscriptArg::Keyword { name, value } => {
                        target.operand_of(*value).map(|n| (n, Some(name.clone())))
                    }
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?,
            _ => return None,
        };
        let (receiver, receiver_place) = self.lower_call_receiver(object);
        let (receiver, receiver_place) =
            self.address_receiver(&plan.getter, object, (receiver, receiver_place));
        let (mut positional, mut keyword) = (0, 0);
        let indices = index_nodes
            .into_iter()
            .map(|(node, name)| {
                let source = if name.is_some() {
                    keyword += 1;
                    CheckedCallArgumentSource::Keyword(keyword - 1)
                } else {
                    positional += 1;
                    CheckedCallArgumentSource::Positional(positional - 1)
                };
                let retain = Self::checked_call_source_requires_place(&plan.getter, source)
                    || Self::checked_call_source_requires_place(&plan.setter, source);
                let (raw, place) = self.lower_augmented_argument_source(node, retain);
                WriteBackIndex {
                    node,
                    source,
                    name,
                    raw,
                    place,
                }
            })
            .collect::<Vec<_>>();
        self.begin_awaited_setup_read(target);
        let operands = indices
            .iter()
            .map(|index| {
                MirSubscriptArg::Index(self.apply_checked_call_value_adjustments(
                    &plan.getter,
                    index.source,
                    index.raw,
                    index.node.location(),
                ))
            })
            .collect::<Vec<_>>();
        let getter_call = self.mir_subscript_call_contract(
            plan.getter.clone(),
            &evaluated_indices(&indices, &operands),
            target.location(),
        );
        let (args, kwargs) = split_keywords(&indices, operands);
        self.emit_checked_call_boundary(&plan.getter, &target.location());
        let current = self.fresh_typed(target.location(), None, plan.getter.result_ty.clone());
        self.record_call(current, Some(getter_call.target.clone()), 0);
        let call = Some(Box::new(getter_call));
        self.emit(match (target.op(), args.as_slice()) {
            (CheckedOp::Index { .. }, [MirSubscriptArg::Index(index)]) if kwargs.is_empty() => {
                MirInstr::Index {
                    dest: current,
                    base: receiver,
                    index: *index,
                    call,
                    intrinsic: None,
                }
            }
            _ => MirInstr::MultiIndex {
                dest: current,
                object: receiver,
                args,
                kwargs,
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
        let setter_receiver = self.setter_receiver(
            setter,
            object,
            receiver,
            receiver_place.as_ref(),
            matches!(
                plan.getter.receiver_convention,
                Some(mojito_ast::ast::ArgConvention::Mut | mojito_ast::ast::ArgConvention::Ref)
            ),
        );
        let mut operands = Vec::with_capacity(indices.len());
        for index in &indices {
            let raw = self.reload_augmented_source(
                index.raw,
                &index.place,
                Self::checked_call_source_mutates(&plan.getter, index.source),
                index.node.location(),
            );
            operands.push(MirSubscriptArg::Index(
                self.apply_checked_call_value_adjustments(
                    setter,
                    index.source,
                    raw,
                    index.node.location(),
                ),
            ));
        }
        // The value is the setter's last keyword, after any keyword index.
        let keywords = indices.iter().filter(|index| index.name.is_some()).count();
        let (value, _) = self.augmented_setter_value(
            target,
            setter,
            CheckedCallArgumentSource::Keyword(keywords),
            CValue::Slot(slot),
        );
        let setter_call = self.mir_subscript_call_contract(
            setter.clone(),
            &evaluated_indices(&indices, &operands),
            target.location(),
        );
        let (args, kwargs) = split_keywords(&indices, operands);
        self.emit_checked_call_boundary(setter, &target.location());
        self.emit(MirInstr::MultiSet {
            receiver: setter_receiver,
            args,
            kwargs,
            value,
            value_keyword: plan.value_keyword.clone(),
            call: Box::new(setter_call),
        });
    }
}

/// The adjusted index `operands` of a write-back, each paired with the
/// source its `indices` entry evaluated.
fn evaluated_indices(
    indices: &[WriteBackIndex<'_>],
    operands: &[MirSubscriptArg],
) -> Vec<super::facts::SubscriptOperand> {
    indices
        .iter()
        .zip(operands)
        .map(|(index, operand)| (Some(index.node.location()), operand.clone()))
        .collect()
}

/// A write-back's adjusted index `operands`, aligned with `indices`, split
/// into the positional list and the keyword list a subscript instruction
/// carries.
fn split_keywords(
    indices: &[WriteBackIndex<'_>],
    operands: Vec<MirSubscriptArg>,
) -> (Vec<MirSubscriptArg>, Vec<(String, MirSubscriptArg)>) {
    let mut args = Vec::new();
    let mut kwargs = Vec::new();
    for (index, operand) in indices.iter().zip(operands) {
        match &index.name {
            Some(name) => kwargs.push((name.clone(), operand)),
            None => args.push(operand),
        }
    }
    (args, kwargs)
}

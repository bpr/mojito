//! The storage each call operand addresses.
//!
//! Upstream's `lit.call` takes an argument its callee receives by address as
//! a `!lit.ref` SSA value, and every later reader — origin and exclusivity
//! checking, liveness, the inliner — works from that operand. MIR lowering
//! forms the same operand, a `ref.make` of the operand's place
//! (`Flatten::address_operand`), so a reader after lowering asks
//! [`OperandAddresses::address`] for the storage an operand addresses instead
//! of the instruction's place table.
//!
//! Registers are function-wide unique and defined once, so one map over a
//! function serves every call in it, wherever CSE or hoisting moved the
//! `ref.make` an operand is.

use crate::mir::{MirBlock, MirInstr, MirPlace, MirSubscriptArg, Reg, VarId, instruction_regions};
use mojito_types::types::{Ty, TyKind};
use std::collections::HashMap;
use std::hash::BuildHasher;

/// Which of a call-shaped instruction's operands a [`CallOperand`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandSlot {
    /// A `call.method`'s receiver, an `index.get`'s base, an
    /// `index.multi`'s object, or an `index.multi_set`'s receiver.
    Receiver,
    /// A `call.indirect`'s callee value.
    Callee,
    /// The positional operand at this index.
    Positional(usize),
    /// The keyword operand at this index.
    Keyword(usize),
    /// An `index.get`'s index.
    Index,
    /// An `index.multi_set`'s stored value.
    Value,
}

/// One operand of a call-shaped instruction, with the entry its place table
/// carries beside it.
#[derive(Debug, Clone, Copy)]
pub struct CallOperand<'i> {
    pub reg: Reg,
    pub slot: OperandSlot,
    pub table: Option<&'i MirPlace>,
}

/// The place each `ref.make` of a function defines its register to address.
#[derive(Debug, Default)]
pub struct OperandAddresses(HashMap<u32, MirPlace>);

impl OperandAddresses {
    /// The storage `operand` addresses: the place of the `ref.make` its
    /// register is, as upstream reads a `!lit.ref` operand.
    ///
    /// An operand that is no `ref.make` reads the place table instead
    /// ([`table_fallback`]).
    #[must_use]
    pub fn address<'a>(&'a self, operand: &CallOperand<'a>) -> Option<&'a MirPlace> {
        self.made(operand.reg).or_else(|| table_fallback(operand))
    }

    /// The place `reg`'s defining `ref.make` addresses, when it is one.
    #[must_use]
    pub fn made(&self, reg: Reg) -> Option<&MirPlace> {
        self.0.get(&reg.0)
    }

    /// Add every `ref.make` of `body` that forms a reference handle, as
    /// [`operand_addresses`] collects them: the ones a pass introduced
    /// after the map was built, such as a body the inliner spliced.
    pub fn extend<S: BuildHasher>(
        &mut self,
        body: &MirBlock,
        register_types: &HashMap<u32, Ty, S>,
    ) {
        collect(body, register_types, &mut self.0);
    }

    /// The storage each operand of `instruction` addresses, by slot.
    #[must_use]
    pub fn operands<'a>(&'a self, instruction: &'a MirInstr) -> AddressedOperands<'a> {
        let mut addressed = AddressedOperands::default();
        for operand in call_operands(instruction) {
            let address = self.address(&operand);
            match operand.slot {
                OperandSlot::Receiver => addressed.receiver = address,
                OperandSlot::Callee => addressed.callee = address,
                OperandSlot::Positional(_) => addressed.positional.push(address),
                OperandSlot::Keyword(_) => addressed.keyword.push(address),
                OperandSlot::Index => addressed.index = address,
                OperandSlot::Value => addressed.value = address,
            }
        }
        addressed
    }
}

/// The storage each operand of one call-shaped instruction addresses
/// ([`OperandAddresses::operands`]), aligned with its operand lists.
#[derive(Debug, Default)]
pub struct AddressedOperands<'a> {
    pub receiver: Option<&'a MirPlace>,
    pub callee: Option<&'a MirPlace>,
    pub positional: Vec<Option<&'a MirPlace>>,
    pub keyword: Vec<Option<&'a MirPlace>>,
    pub index: Option<&'a MirPlace>,
    pub value: Option<&'a MirPlace>,
}

/// Every `ref.make` of `body`, its regions included, that forms a
/// reference handle (`register_types` types its register a `ref`), by the
/// register it defines.
///
/// One typed a `!lit.ref` is a value the program spells
/// (`__get_mvalue_as_litref`), which its callee receives by value.
#[must_use]
pub fn operand_addresses<S: BuildHasher>(
    body: &MirBlock,
    register_types: &HashMap<u32, Ty, S>,
) -> OperandAddresses {
    let mut addresses = OperandAddresses::default();
    addresses.extend(body, register_types);
    addresses
}

/// The slot each register of `body`, its regions included, was read from
/// whole.
///
/// That is the slot of a `var.use`, of a whole-root `place.load` not
/// through a reference, or of a `ref.read` of a `ref.make` addressing such
/// a root.
///
/// A `call.indirect` takes its callee as the operand alone, as upstream's
/// indirect call takes the closure or function value; a pass that wants the
/// storage the value was read from follows the operand's defining load, as
/// an upstream pass reaches the alloca through it.
#[must_use]
pub fn callee_slots(body: &MirBlock) -> HashMap<u32, VarId> {
    let mut loads = Loads::default();
    loads.collect(body);
    let Loads {
        mut slots,
        roots,
        reads,
    } = loads;
    slots.extend(
        reads
            .into_iter()
            .filter_map(|(dest, reference)| Some((dest, *roots.get(&reference)?))),
    );
    slots
}

/// The operands of a call-shaped instruction, each with its place-table
/// entry.
///
/// The call-shaped instructions are `call`, `call.indirect`,
/// `call.method`, `index.get`, `index.multi`, `index.multi_set`, and
/// `type.construct_param`; any other instruction has none.
#[must_use]
pub fn call_operands(instruction: &MirInstr) -> Vec<CallOperand<'_>> {
    let mut operands = Vec::new();
    match instruction {
        MirInstr::Call {
            args,
            kwargs,
            arg_places,
            kwarg_places,
            ..
        } => {
            push_positional(&mut operands, args.iter().copied(), arg_places);
            push_keywords(
                &mut operands,
                kwargs.iter().map(|(_, reg)| *reg),
                kwarg_places,
            );
        }
        MirInstr::CallIndirect {
            callee,
            args,
            kwargs,
            arg_places,
            kwarg_places,
            ..
        } => {
            operands.push(CallOperand {
                reg: *callee,
                slot: OperandSlot::Callee,
                table: None,
            });
            push_positional(&mut operands, args.iter().copied(), arg_places);
            push_keywords(
                &mut operands,
                kwargs.iter().map(|(_, reg)| *reg),
                kwarg_places,
            );
        }
        MirInstr::MethodCall {
            recv,
            recv_place,
            args,
            kwargs,
            arg_places,
            kwarg_places,
            ..
        } => {
            operands.push(CallOperand {
                reg: *recv,
                slot: OperandSlot::Receiver,
                table: recv_place.as_deref(),
            });
            push_positional(&mut operands, args.iter().copied(), arg_places);
            push_keywords(
                &mut operands,
                kwargs.iter().map(|(_, reg)| *reg),
                kwarg_places,
            );
        }
        MirInstr::Index {
            base,
            index,
            base_place,
            index_place,
            ..
        } => {
            operands.push(CallOperand {
                reg: *base,
                slot: OperandSlot::Receiver,
                table: base_place.as_deref(),
            });
            operands.push(CallOperand {
                reg: *index,
                slot: OperandSlot::Index,
                table: index_place.as_deref(),
            });
        }
        MirInstr::MultiIndex {
            object,
            object_place,
            args,
            arg_places,
            kwargs,
            kwarg_places,
            ..
        } => {
            operands.push(CallOperand {
                reg: *object,
                slot: OperandSlot::Receiver,
                table: object_place.as_deref(),
            });
            push_positional(&mut operands, args.iter().map(subscript_reg), arg_places);
            push_keywords(
                &mut operands,
                kwargs.iter().map(|(_, argument)| subscript_reg(argument)),
                kwarg_places,
            );
        }
        MirInstr::MultiSet {
            receiver,
            receiver_place,
            args,
            arg_places,
            value,
            value_place,
            ..
        } => {
            operands.push(CallOperand {
                reg: *receiver,
                slot: OperandSlot::Receiver,
                table: receiver_place.as_deref(),
            });
            push_positional(&mut operands, args.iter().map(subscript_reg), arg_places);
            operands.push(CallOperand {
                reg: *value,
                slot: OperandSlot::Value,
                table: value_place.as_deref(),
            });
        }
        MirInstr::ConstructTypeParam {
            kwargs,
            kwarg_places,
            ..
        } => push_keywords(
            &mut operands,
            kwargs.iter().map(|(_, reg)| *reg),
            kwarg_places,
        ),
        _ => {}
    }
    operands
}

fn push_positional<'i>(
    operands: &mut Vec<CallOperand<'i>>,
    regs: impl Iterator<Item = Reg>,
    tables: &'i [Option<MirPlace>],
) {
    operands.extend(regs.enumerate().map(|(index, reg)| CallOperand {
        reg,
        slot: OperandSlot::Positional(index),
        table: tables.get(index).and_then(Option::as_ref),
    }));
}

fn push_keywords<'i>(
    operands: &mut Vec<CallOperand<'i>>,
    regs: impl Iterator<Item = Reg>,
    tables: &'i [Option<MirPlace>],
) {
    operands.extend(regs.enumerate().map(|(index, reg)| CallOperand {
        reg,
        slot: OperandSlot::Keyword(index),
        table: tables.get(index).and_then(Option::as_ref),
    }));
}

const fn subscript_reg(argument: &MirSubscriptArg) -> Reg {
    match argument {
        MirSubscriptArg::Index(reg) => *reg,
    }
}

/// The place-table entry of an operand that is no `ref.make`, the shapes
/// whose call still names its storage only in the table:
///
/// - a `var` receiver's `recv_place`;
/// - an operand that is already a reference without being a `ref.make` of
///   its table's place (a forwarded reference parameter).
const fn table_fallback<'a>(operand: &CallOperand<'a>) -> Option<&'a MirPlace> {
    operand.table
}

/// The whole-slot reads of a body, gathered for [`callee_slots`].
#[derive(Default)]
struct Loads {
    slots: HashMap<u32, VarId>,
    roots: HashMap<u32, VarId>,
    reads: Vec<(u32, u32)>,
}

impl Loads {
    fn collect(&mut self, block: &MirBlock) {
        for instruction in &block.instrs {
            match instruction {
                MirInstr::UseVar { dest, var, .. } => {
                    self.slots.insert(dest.0, *var);
                }
                MirInstr::LoadPlace { dest, place } if whole_slot(place) => {
                    self.slots.insert(dest.0, place.root);
                }
                MirInstr::MakeRef { dest, place } if whole_slot(place) => {
                    self.roots.insert(dest.0, place.root);
                }
                MirInstr::ReadRef { dest, reference } => self.reads.push((dest.0, reference.0)),
                _ => {}
            }
            for region in instruction_regions(instruction) {
                self.collect(region);
            }
        }
    }
}

fn whole_slot(place: &MirPlace) -> bool {
    place.is_whole_root() && place.through.is_none()
}

fn collect<S: BuildHasher>(
    block: &MirBlock,
    register_types: &HashMap<u32, Ty, S>,
    addresses: &mut HashMap<u32, MirPlace>,
) {
    for instruction in &block.instrs {
        if let MirInstr::MakeRef { dest, place } = instruction
            && register_types
                .get(&dest.0)
                .is_some_and(|ty| matches!(ty.kind(), TyKind::Ref(_)))
        {
            addresses.insert(dest.0, place.clone());
        }
        for region in instruction_regions(instruction) {
            collect(region, register_types, addresses);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::callee_slots;
    use crate::mir::{MirBlock, MirInstr, MirPlace, MirTerm, Reg, UseMode};
    use mojito_types::types::Ty;

    /// A callee read by a shared borrow of its slot, and one a lambda
    /// temporary's whole-root load reads, each resolve to their slot; a
    /// projected load does not.
    #[test]
    fn a_callee_resolves_to_the_slot_its_defining_load_reads() {
        let mut field = MirPlace::root(4, Some(Ty::int()));
        field.proj.push(crate::mir::Proj::Field("f".into()));
        let block = MirBlock {
            instrs: vec![
                MirInstr::UseVar {
                    dest: Reg(0),
                    var: 2,
                    mode: UseMode::BorrowShared,
                    constructor: None,
                },
                MirInstr::LoadPlace {
                    dest: Reg(1),
                    place: MirPlace::root(3, Some(Ty::int())),
                },
                MirInstr::LoadPlace {
                    dest: Reg(2),
                    place: field,
                },
            ],
            term: MirTerm::TryYield(Vec::new()),
        };
        let slots = callee_slots(&block);
        assert_eq!(slots.get(&0), Some(&2));
        assert_eq!(slots.get(&1), Some(&3));
        assert_eq!(slots.get(&2), None);
    }
}

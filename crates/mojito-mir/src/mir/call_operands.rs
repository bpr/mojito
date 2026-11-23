//! The storage each call operand addresses.
//!
//! Upstream's `lit.call` takes an argument its callee receives by address as
//! a `!lit.ref<P>` SSA value at a parameter that takes `P`, and every later
//! reader — origin and exclusivity checking, liveness, the inliner — works
//! from that operand. MIR lowering forms the same operand, a `ref.make` of
//! the operand's place (`Flatten::address_operand`), so a reader after
//! lowering asks [`OperandAddresses::address`] for the storage an operand
//! addresses; the instruction keeps no other record of it. Which operands
//! are by address is read off each call's contract, as `LowerArgConventions`
//! reads it ([`crate::mir::addressed_registers`]): a `!lit.ref` value a
//! callee takes at a `!lit.ref` parameter (`__get_mvalue_as_litref`, a
//! parameter list's address) is passed by value and addresses no storage.
//!
//! Registers are function-wide unique and defined once, so one map over a
//! function serves every call in it, wherever CSE or hoisting moved the
//! `ref.make` an operand is.

use crate::mir::call_addresses::addressed_registers_in;
use crate::mir::{
    MirBlock, MirDeclarations, MirFunction, MirInstr, MirPlace, MirSubscriptArg, Reg, VarId,
    instruction_regions,
};
use std::collections::{HashMap, HashSet};

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

/// One operand of a call-shaped instruction, by the slot it fills.
#[derive(Debug, Clone, Copy)]
pub struct CallOperand {
    pub reg: Reg,
    pub slot: OperandSlot,
}

impl CallOperand {
    const fn new(reg: Reg, slot: OperandSlot) -> Self {
        Self { reg, slot }
    }
}

/// The place each `ref.make` a call of a function passes as a by-address
/// operand addresses, by the register the `ref.make` defines.
#[derive(Debug, Default)]
pub struct OperandAddresses(HashMap<u32, MirPlace>);

impl OperandAddresses {
    /// The storage `operand` addresses: the place of the `ref.make` its
    /// register is, as upstream reads a `!lit.ref` operand. An operand that
    /// is no `ref.make` addresses none.
    #[must_use]
    pub fn address(&self, operand: &CallOperand) -> Option<&MirPlace> {
        self.made(operand.reg)
    }

    /// The place `reg`'s defining `ref.make` addresses, when it is one.
    #[must_use]
    pub fn made(&self, reg: Reg) -> Option<&MirPlace> {
        self.0.get(&reg.0)
    }

    /// Add every by-address `ref.make` operand of a call in `body`, whose
    /// registers `function` types, as [`operand_addresses`] collects them:
    /// the ones a pass introduced after the map was built, such as a body
    /// the inliner spliced into `function`.
    pub fn extend(
        &mut self,
        body: &MirBlock,
        function: &MirFunction,
        declarations: &MirDeclarations,
    ) {
        let addressed = addressed_registers_in(body, &function.reg_types, declarations);
        collect(body, &addressed, &mut self.0);
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

/// Every `ref.make` of `function`, its regions included, that a call passes
/// as a by-address operand, by the register it defines.
///
/// A by-address operand is a `ref<P>` at a parameter taking `P`, read off
/// the call's contract ([`crate::mir::addressed_registers`]).
#[must_use]
pub fn operand_addresses(
    function: &MirFunction,
    declarations: &MirDeclarations,
) -> OperandAddresses {
    let mut addresses = OperandAddresses::default();
    addresses.extend(&function.body, function, declarations);
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

/// The operands of a call-shaped instruction.
///
/// The call-shaped instructions are `call`, `call.indirect`,
/// `call.method`, `index.get`, `index.multi`, `index.multi_set`, and
/// `type.construct_param`; any other instruction has none.
#[must_use]
pub fn call_operands(instruction: &MirInstr) -> Vec<CallOperand> {
    let mut operands = Vec::new();
    match instruction {
        MirInstr::Call { args, kwargs, .. } => {
            push_positional(&mut operands, args.iter().copied());
            push_keywords(&mut operands, kwargs.iter().map(|(_, reg)| *reg));
        }
        MirInstr::CallIndirect {
            callee,
            args,
            kwargs,
            ..
        } => {
            operands.push(CallOperand::new(*callee, OperandSlot::Callee));
            push_positional(&mut operands, args.iter().copied());
            push_keywords(&mut operands, kwargs.iter().map(|(_, reg)| *reg));
        }
        MirInstr::MethodCall {
            recv, args, kwargs, ..
        } => {
            operands.push(CallOperand::new(*recv, OperandSlot::Receiver));
            push_positional(&mut operands, args.iter().copied());
            push_keywords(&mut operands, kwargs.iter().map(|(_, reg)| *reg));
        }
        MirInstr::Index { base, index, .. } => {
            operands.push(CallOperand::new(*base, OperandSlot::Receiver));
            operands.push(CallOperand::new(*index, OperandSlot::Index));
        }
        MirInstr::MultiIndex {
            object,
            args,
            kwargs,
            ..
        } => {
            operands.push(CallOperand::new(*object, OperandSlot::Receiver));
            push_positional(&mut operands, args.iter().map(subscript_reg));
            push_keywords(
                &mut operands,
                kwargs.iter().map(|(_, argument)| subscript_reg(argument)),
            );
        }
        MirInstr::MultiSet {
            receiver,
            args,
            kwargs,
            value,
            ..
        } => {
            operands.push(CallOperand::new(*receiver, OperandSlot::Receiver));
            push_positional(&mut operands, args.iter().map(subscript_reg));
            push_keywords(
                &mut operands,
                kwargs.iter().map(|(_, argument)| subscript_reg(argument)),
            );
            operands.push(CallOperand::new(*value, OperandSlot::Value));
        }
        MirInstr::ConstructTypeParam { kwargs, .. } => {
            push_keywords(&mut operands, kwargs.iter().map(|(_, reg)| *reg));
        }
        _ => {}
    }
    operands
}

fn push_positional(operands: &mut Vec<CallOperand>, regs: impl Iterator<Item = Reg>) {
    operands.extend(
        regs.enumerate()
            .map(|(index, reg)| CallOperand::new(reg, OperandSlot::Positional(index))),
    );
}

fn push_keywords(operands: &mut Vec<CallOperand>, regs: impl Iterator<Item = Reg>) {
    operands.extend(
        regs.enumerate()
            .map(|(index, reg)| CallOperand::new(reg, OperandSlot::Keyword(index))),
    );
}

const fn subscript_reg(argument: &MirSubscriptArg) -> Reg {
    match argument {
        MirSubscriptArg::Index(reg) => *reg,
    }
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

fn collect(block: &MirBlock, addressed: &HashSet<u32>, addresses: &mut HashMap<u32, MirPlace>) {
    for instruction in &block.instrs {
        if let MirInstr::MakeRef { dest, place } = instruction
            && addressed.contains(&dest.0)
        {
            addresses.insert(dest.0, place.clone());
        }
        for region in instruction_regions(instruction) {
            collect(region, addressed, addresses);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{callee_slots, operand_addresses};
    use crate::mir::{
        FuncRef, MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr,
        MirPlace, MirTerm, Reg, UseMode,
    };
    use mojito_ast::ast::ArgConvention;
    use mojito_types::origin::{Mutability, Origin, RefTy};
    use mojito_types::types::{AddressSpaceSlot, Ty, TyKind};

    fn reference(referent: Ty) -> Ty {
        TyKind::Ref(RefTy {
            referent,
            origin: Origin::Static,
            mutability: Mutability::Mutable,
            address_space: AddressSpaceSlot::GENERIC,
        })
        .into()
    }

    /// `take(mut by_address: Int, by_value: ref Int)`.
    fn take_declarations() -> MirDeclarations {
        let declaration = MirFunctionDeclaration {
            lowered_name: "take".into(),
            param_names: vec!["by_address".into(), "by_value".into()],
            param_types: vec![Ty::int(), reference(Ty::int())],
            defaults: vec![None; 2],
            required: vec![true; 2],
            variadic: None,
            variadic_convention: None,
            variadic_index: None,
            kw_variadic: None,
            kw_variadic_convention: None,
            kw_variadic_index: None,
            positional_only: None,
            keyword_only: None,
            param_decls: Vec::new(),
            has_receiver: false,
            receiver_convention: None,
            param_conventions: vec![Some(ArgConvention::Mut), None],
            ret_ty: Ty::none(),
            returns_reference: false,
            raises: false,
            error_ty: None,
            ref_params: vec![true, false],
            param_writes: vec![true, false],
            availability: Vec::new(),
            enclosing: None,
            region_binders: Vec::new(),
            inline: mojito_checked::declaration::InlineLevel::NoDebug,
            provenance: mojito_checked::declaration::FunctionProvenance::Synthesized,
        };
        MirDeclarations {
            functions: vec![declaration],
            ..MirDeclarations::default()
        }
    }

    /// Two `ref.make`s of the same reference type: the one `take` takes by
    /// address at its `Int` parameter addresses its place, and the one it
    /// takes as a reference value at its reference parameter addresses
    /// none, as `LowerArgConventions` reads the call.
    #[test]
    fn only_a_by_address_operand_addresses_its_place() {
        let block = MirBlock {
            instrs: vec![
                MirInstr::MakeRef {
                    dest: Reg(0),
                    place: MirPlace::root(0, Some(Ty::int())),
                },
                MirInstr::MakeRef {
                    dest: Reg(1),
                    place: MirPlace::root(1, Some(Ty::int())),
                },
                MirInstr::Call {
                    conventions: crate::mir::MirArgConventions::default(),
                    dest: Reg(2),
                    func: FuncRef("take".into()),
                    raises: None,
                    args: vec![Reg(0), Reg(1)],
                    kwargs: Vec::new(),
                    capture_accesses: Vec::new(),
                    param_arg_regs: Vec::new(),
                    receiver: None,
                    instantiated_args: Vec::new(),
                    spread: None,
                },
            ],
            term: MirTerm::Return(None),
        };
        let function = MirFunction {
            body: block,
            reg_types: [(0, reference(Ty::int())), (1, reference(Ty::int()))]
                .into_iter()
                .collect(),
            ..MirFunction::default()
        };
        let addresses = operand_addresses(&function, &take_declarations());
        assert_eq!(addresses.made(Reg(0)).map(|place| place.root), Some(0));
        assert!(addresses.made(Reg(1)).is_none());
    }

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

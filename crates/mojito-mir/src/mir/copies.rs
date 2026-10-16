//! The copy and move constructors a body runs without naming them.
//!
//! Upstream's parser and its move/copy insertion leave a call to
//! `__copyinit__` or `__moveinit__` at each point a value is copied or
//! moved, so a symbol walk reaches it. Here a copy or a move is a register
//! read (`var.use copy`, `value.copy`, a load out of storage), and the VM
//! and the native backend find the struct's member from the value's type.
//! This module answers which structs those are: the copied and moved
//! types, and every field type a copy or a move of them may reach.

use super::destroys::close_over_fields;
use crate::mir::verify::{instruction_operand_regs, instruction_result_regs};
use crate::mir::{
    MirBlock, MirCaptureMode, MirFunction, MirInstr, MirPlace, Reg, StructFields, UseMode,
    instruction_regions,
};
use mojito_hir::hir::VarId;
use mojito_types::types::Ty;
use std::collections::HashMap;

/// What a body copies and moves without naming a constructor.
pub struct Relocated {
    /// The struct types whose `__copyinit__` it may run.
    pub copied: Vec<Ty>,
    /// The struct types whose `__moveinit__` it may run.
    pub moved: Vec<Ty>,
    /// Whether it copies or moves a value of a type only an instantiation
    /// resolves, whose members no walk of a generic body can name.
    pub symbolic: bool,
}

/// What the body `function` copies and moves without naming a constructor.
///
/// A copy is a `var.use copy`, a `value.copy`, an owned closure capture, a
/// variant payload, a `Type(copy=value)` construction, a method result
/// read through its reference, a borrowed value stored into owning
/// storage, and any read whose result a backend may clone out of storage
/// (a load, a field, an element, an iterator's next value). A move is a
/// `var.use move` and an owned closure capture. Through `fields`, every
/// field of a struct those reach joins, since a struct without the member
/// copies or moves its fields one by one.
pub fn relocated_structs(function: &MirFunction, fields: &StructFields<'_>) -> Relocated {
    let mut walk = Walk {
        function,
        borrowed: HashMap::new(),
        copied: Vec::new(),
        moved: Vec::new(),
    };
    walk.borrows(&function.blocks);
    walk.blocks(&function.blocks);
    let copied = close_over_fields(walk.copied, fields);
    let moved = close_over_fields(walk.moved, fields);
    Relocated {
        copied: copied.structs,
        moved: moved.structs,
        symbolic: copied.symbolic || moved.symbolic,
    }
}

/// The types a body's instructions copy and move.
struct Walk<'f> {
    function: &'f MirFunction,
    /// Each register a shared or mutable borrow of a slot defines.
    borrowed: HashMap<u32, VarId>,
    copied: Vec<(Ty, bool)>,
    moved: Vec<(Ty, bool)>,
}

impl Walk<'_> {
    fn borrows(&mut self, blocks: &[MirBlock]) {
        for block in blocks {
            for instruction in &block.instrs {
                if let MirInstr::UseVar {
                    dest,
                    var,
                    mode: UseMode::BorrowShared | UseMode::BorrowMut,
                } = instruction
                {
                    self.borrowed.insert(dest.0, *var);
                }
                for region in instruction_regions(instruction) {
                    self.borrows(region);
                }
            }
        }
    }

    fn blocks(&mut self, blocks: &[MirBlock]) {
        for block in blocks {
            for instruction in &block.instrs {
                self.instruction(instruction);
                for region in instruction_regions(instruction) {
                    self.blocks(region);
                }
            }
        }
    }

    fn instruction(&mut self, instruction: &MirInstr) {
        match instruction {
            MirInstr::UseVar { var, mode, .. } => match mode {
                UseMode::Copy => self.var(*var, false),
                UseMode::Move => self.var(*var, true),
                UseMode::BorrowShared | UseMode::BorrowMut | UseMode::Forward => {}
            },
            // A closure value copies its owned captures with itself.
            MirInstr::MakeClosure { captures, .. } => {
                for capture in captures {
                    if capture.mode != MirCaptureMode::Reference {
                        self.place(&capture.place, false);
                        self.place(&capture.place, true);
                    }
                }
            }
            MirInstr::MakeVariant { value, .. }
            | MirInstr::VariantSet { value, .. }
            | MirInstr::VariantReplace { value, .. } => self.register(*value),
            MirInstr::VariantSetInitWith { place, .. } => self.place(place, false),
            MirInstr::MethodCall {
                dest,
                result_adapter: Some(_),
                ..
            } => self.register(*dest),
            MirInstr::Call {
                dest, args, kwargs, ..
            } if args.is_empty()
                && matches!(kwargs.as_slice(), [(keyword, _)] if keyword == "copy") =>
            {
                self.register(*dest);
            }
            // Fresh values, and reads that relocate rather than clone.
            MirInstr::MakeRef { .. }
            | MirInstr::Const { .. }
            | MirInstr::SizeOf { .. }
            | MirInstr::ParamListAddress { .. }
            | MirInstr::TypeName { .. }
            | MirInstr::ConstructTypeParam { .. }
            | MirInstr::ConstructType { .. }
            | MirInstr::MaterializeLiteral { .. }
            | MirInstr::StoreToMem { .. }
            | MirInstr::MovePlace { .. }
            | MirInstr::UnOp { .. }
            | MirInstr::BinOp { .. }
            | MirInstr::Call { .. }
            | MirInstr::CallIndirect { .. }
            | MirInstr::MethodCall { .. }
            | MirInstr::PointerStorageTake { .. }
            | MirInstr::PointerStorageDestroy { .. }
            | MirInstr::UninitStorageTake { .. }
            | MirInstr::UninitStorageDestroy { .. }
            | MirInstr::MakeSimd { .. }
            | MirInstr::SimdCast { .. }
            | MirInstr::SimdBitcast { .. }
            | MirInstr::SimdShuffle { .. }
            | MirInstr::VariantIs { .. }
            | MirInstr::VariantTake { .. }
            | MirInstr::VariantDeinitWith { .. }
            | MirInstr::HasNext { .. }
            | MirInstr::EstablishLoans { .. }
            | MirInstr::InvalidateInteriors { .. }
            | MirInstr::ConsumePlace { .. } => {}
            _ => {
                let mut results = Vec::new();
                instruction_result_regs(instruction, &mut results);
                for register in results {
                    self.register(register);
                }
            }
        }
        // A borrowed value stored into owning storage is cloned there.
        if matches!(
            instruction,
            MirInstr::Store { .. }
                | MirInstr::WriteRef { .. }
                | MirInstr::MakeTuple { .. }
                | MirInstr::MakeVariant { .. }
                | MirInstr::VariantSet { .. }
                | MirInstr::VariantReplace { .. }
                | MirInstr::UninitStorage { .. }
                | MirInstr::MultiSet { .. }
        ) {
            let mut operands = Vec::new();
            instruction_operand_regs(instruction, &mut operands);
            for register in operands {
                if let Some(&var) = self.borrowed.get(&register.0) {
                    self.var(var, false);
                }
            }
        }
    }

    fn register(&mut self, register: Reg) {
        if let Some(ty) = self.function.reg_types.get(&register.0) {
            self.copied.push((ty.clone(), true));
        }
    }

    fn var(&mut self, var: VarId, moved: bool) {
        if let Some(ty) = self.function.var_tys.get(&var) {
            self.push(ty.clone(), moved);
        }
    }

    fn place(&mut self, place: &MirPlace, moved: bool) {
        match place
            .ty
            .as_ref()
            .or_else(|| place.projection_tys.last())
            .or_else(|| place.root_ty.as_ref().filter(|_| place.proj.is_empty()))
        {
            Some(ty) => self.push(ty.clone(), moved),
            None if place.proj.is_empty() => self.var(place.root, moved),
            None => {}
        }
    }

    fn push(&mut self, ty: Ty, moved: bool) {
        if moved {
            self.moved.push((ty, true));
        } else {
            self.copied.push((ty, true));
        }
    }
}

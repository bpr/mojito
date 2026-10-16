//! The copy and move constructors a body runs without naming them.
//!
//! Upstream's parser and its move/copy insertion leave a call to
//! `__copyinit__` or `__moveinit__` at each point a value is copied or
//! moved, so a symbol walk reaches it. A `var.use copy`, `var.use move`, or
//! `value.copy` names the constructor it runs, which a symbol walk
//! follows; the rest of what a body copies or moves (a site naming none, a
//! capture, a payload) names none, and the VM and
//! the native backend find the struct's member from the value's type. This
//! module answers which structs those are: the types copied and moved
//! without a name, and every field type a copy or a move of them may
//! reach.

use super::destroys::close_over_fields;
use crate::mir::{
    MirBlock, MirCaptureMode, MirFunction, MirInstr, MirPlace, Reg, StructFields, UseMode,
    instruction_regions,
};
use mojito_hir::hir::VarId;
use mojito_types::types::Ty;

/// What a body copies and moves without naming a constructor.
pub struct Relocated {
    /// The struct types whose `__copyinit__` it may run.
    pub copied: Vec<Ty>,
    /// The struct types whose `__moveinit__` it may run.
    pub moved: Vec<Ty>,
    /// Whether it copies or moves a value of a type only an instantiation
    /// resolves, whose members no walk of a generic body can name.
    pub symbolic: bool,
    /// Each type a copy or move names its constructor for, beside it.
    pub named: Vec<(Ty, String)>,
}

/// What the body `function` copies and moves without naming a constructor.
///
/// A copy is a `var.use copy`, a `value.copy`, an owned closure capture, a
/// variant payload, a `Type(copy=value)` construction, and a method or
/// iterator result read through its reference: the points the VM and the native backend
/// copy at. A read out of storage (a load, a field, an element) and a
/// store copy nothing. A move is a `var.use move` and an owned closure
/// capture. Through `fields`, every
/// field of a struct those reach joins, since a struct without the member
/// copies or moves its fields one by one. A copy or move naming its
/// constructor runs it instead, and joins [`Relocated::named`].
pub fn relocated_structs(function: &MirFunction, fields: &StructFields<'_>) -> Relocated {
    let mut walk = Walk {
        function,
        copied: Vec::new(),
        moved: Vec::new(),
        named: Vec::new(),
    };
    walk.blocks(&function.blocks);
    let copied = close_over_fields(walk.copied, fields);
    let moved = close_over_fields(walk.moved, fields);
    Relocated {
        copied: copied.structs,
        moved: moved.structs,
        symbolic: copied.symbolic || moved.symbolic,
        named: walk.named,
    }
}

/// The types a body's instructions copy and move.
struct Walk<'f> {
    function: &'f MirFunction,
    copied: Vec<(Ty, bool)>,
    moved: Vec<(Ty, bool)>,
    named: Vec<(Ty, String)>,
}

impl Walk<'_> {
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
            // A site naming its constructor reaches it by symbol.
            MirInstr::UseVar {
                var,
                constructor: Some(constructor),
                ..
            } => self.named(self.function.var_tys.get(var), constructor),
            MirInstr::CopyValue {
                dest,
                constructor: Some(constructor),
                ..
            } => self.named(self.function.reg_types.get(&dest.0), constructor),
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
            MirInstr::TryNext { dest, call, .. } if call.result_adapter.is_some() => {
                self.register(*dest);
            }
            MirInstr::Call {
                dest, args, kwargs, ..
            } if args.is_empty()
                && matches!(kwargs.as_slice(), [(keyword, _)] if keyword == "copy") =>
            {
                self.register(*dest);
            }
            MirInstr::CopyValue { dest, .. } => self.register(*dest),
            // Every other instruction, a read out of storage included,
            // copies nothing.
            _ => {}
        }
    }

    fn named(&mut self, ty: Option<&Ty>, constructor: &str) {
        if let Some(ty) = ty {
            self.named.push((ty.clone(), constructor.to_owned()));
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

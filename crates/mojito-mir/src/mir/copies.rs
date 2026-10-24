//! The copy and move constructors a body runs without naming them.
//!
//! Upstream's parser and its move/copy insertion leave a call to
//! `__copyinit__` or `__moveinit__` at each point a value is copied or
//! moved, so a symbol walk reaches it. Every copy and move site names the
//! constructor it runs, which a symbol walk follows; a site naming none (a
//! type recording no such member) runs none by name, and the VM and the native backend find the struct's member
//! from the value's type. This module answers which structs those are: the
//! types copied and moved without a name, and every field type a copy or a
//! move of a struct without the member may reach.

use crate::mir::{
    MirBlock, MirCaptureMode, MirFunction, MirInstr, MirPlace, MirStructDeclaration, Reg, UseMode,
    instruction_regions,
};
use mojito_hir::hir::VarId;
use mojito_types::types::{Ty, TyKind};
use std::collections::HashSet;

/// A struct's declaration, by the struct's name.
pub type StructDeclarations<'d> = dyn Fn(&str) -> Option<&'d MirStructDeclaration> + 'd;

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
/// variant payload, and a method or iterator result read through its
/// reference: the points the VM and the native backend copy at. A read out
/// of storage (a load, a field, an element) and a store copy nothing. A move is a `var.use move` and an
/// owned closure capture. Through `structs`, every field of a struct those
/// reach *without* the member joins, since such a struct copies or moves
/// its fields one by one; a struct recording its own `__copyinit__` or
/// `__moveinit__` reaches its fields through that member's body. A copy
/// or move naming its constructor runs it instead, and joins
/// [`Relocated::named`].
pub fn relocated_structs(function: &MirFunction, structs: &StructDeclarations<'_>) -> Relocated {
    let mut walk = Walk {
        function,
        copied: Vec::new(),
        moved: Vec::new(),
        named: Vec::new(),
    };
    walk.blocks(&function.blocks);
    let (copied, copied_symbolic) = close_over_fields(walk.copied, "__copyinit__", structs);
    let (moved, moved_symbolic) = close_over_fields(walk.moved, "__moveinit__", structs);
    Relocated {
        copied,
        moved,
        symbolic: copied_symbolic || moved_symbolic,
        named: walk.named,
    }
}

/// The struct types the values `pending` reach, and whether one is
/// symbolic: a struct's fields are reached through `structs` only where
/// the struct records no `member`, as is each element of a tuple, pack,
/// or variant.
fn close_over_fields(
    mut pending: Vec<Ty>,
    member: &str,
    structs: &StructDeclarations<'_>,
) -> (Vec<Ty>, bool) {
    let mut seen = HashSet::new();
    let mut reached = Vec::new();
    let mut symbolic = false;
    while let Some(ty) = pending.pop() {
        if !seen.insert(ty.clone()) {
            continue;
        }
        match ty.kind() {
            TyKind::Struct(name, _) => {
                let declaration = structs(name);
                if declaration.is_none_or(|declaration| declaration.member(member).is_none()) {
                    pending.extend(
                        declaration
                            .into_iter()
                            .flat_map(|declaration| &declaration.fields)
                            .map(|(_, field)| field.clone()),
                    );
                }
                reached.push(ty);
            }
            TyKind::Tuple(items) | TyKind::RuntimePack(items) | TyKind::Variant(items) => {
                pending.extend(items.iter().cloned());
            }
            TyKind::ComptimeList(item)
            | TyKind::VariadicPack(item)
            | TyKind::PopArray { element: item, .. } => pending.push(item.clone()),
            TyKind::Infer
            | TyKind::Param { .. }
            | TyKind::Assoc { .. }
            | TyKind::Dependent(_)
            | TyKind::SelfType => symbolic = true,
            _ => {}
        }
    }
    (reached, symbolic)
}

/// The types a body's instructions copy and move.
struct Walk<'f> {
    function: &'f MirFunction,
    copied: Vec<Ty>,
    moved: Vec<Ty>,
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
            MirInstr::MakeVariant {
                alternatives,
                index,
                constructor: Some(constructor),
                ..
            } => self.named(
                crate::mir::variant_payload_ty(alternatives, index),
                constructor,
            ),
            MirInstr::VariantSet {
                place,
                index,
                constructor: Some(constructor),
                ..
            }
            | MirInstr::VariantReplace {
                place,
                input_index: index,
                constructor: Some(constructor),
                ..
            } => self.named(
                crate::mir::variant_place_payload_ty(place, index, &self.function.var_tys),
                constructor,
            ),
            MirInstr::MethodCall {
                dest,
                result_adapter: Some(adapter),
                ..
            } if adapter.constructor.is_some() => self.named(
                self.function.reg_types.get(&dest.0),
                adapter.constructor.as_deref().unwrap_or_default(),
            ),
            MirInstr::UseVar { var, mode, .. } => match mode {
                UseMode::Copy => self.var(*var, false),
                UseMode::Move => self.var(*var, true),
                UseMode::BorrowShared | UseMode::BorrowMut | UseMode::Forward => {}
            },
            // A closure value copies its owned captures with itself.
            MirInstr::MakeClosure { captures, .. } => {
                for capture in captures {
                    // A copy or move of the closure itself relocates the
                    // capture the other way, finding the member by type.
                    match (&capture.constructor, capture.mode) {
                        (_, MirCaptureMode::Reference) => {}
                        (Some(constructor), mode) => {
                            self.named(
                                capture.place.storage_ty(&self.function.var_tys),
                                constructor,
                            );
                            self.place(&capture.place, mode == MirCaptureMode::Copy);
                        }
                        (None, _) => {
                            self.place(&capture.place, false);
                            self.place(&capture.place, true);
                        }
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
            self.copied.push(ty.clone());
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
            self.moved.push(ty);
        } else {
            self.copied.push(ty);
        }
    }
}

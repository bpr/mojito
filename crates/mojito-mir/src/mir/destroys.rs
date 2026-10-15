//! The destructors a body runs without naming them.
//!
//! Upstream's destructor insertion leaves a call to the destructor at each
//! point a value dies, so a symbol walk reaches it. Here a drop names only
//! the slot or place it tears down, and the VM and the native backend find
//! the struct's `__deinit__` from the value's type. This module answers
//! which structs those are: the dropped types, and every field type a drop
//! of them may tear down field by field.

use crate::mir::{MirBlock, MirFunction, MirInstr, MirPlace, MirTerm, instruction_regions};
use mojito_hir::hir::VarId;
use mojito_types::types::{Ty, TyKind};
use std::collections::{HashMap, HashSet};

/// A struct's declared field types, by the struct's name.
pub type StructFields<'d> = dyn Fn(&str) -> Option<&'d [(String, Ty)]> + 'd;

/// What a body tears down without naming a destructor.
pub struct Destroyed {
    /// The struct types whose `__deinit__` it may run.
    pub structs: Vec<Ty>,
    /// Whether it tears down a value of a type only an instantiation
    /// resolves, whose destructor no walk of a generic body can name.
    pub symbolic: bool,
}

/// What the body `function` destroys without naming a destructor.
///
/// That is a whole value its drops and cleanup lists tear down, the fields
/// of a value it consumes, and, through `fields`, every field of a struct
/// those reach, since a struct without a destructor, or a partly moved one,
/// drops its fields one by one.
pub fn destroyed_structs(function: &MirFunction, fields: &StructFields<'_>) -> Destroyed {
    let mut walk = Walk {
        var_tys: &function.var_tys,
        pending: Vec::new(),
    };
    walk.blocks(&function.blocks);
    let mut pending = walk.pending;
    let mut seen = HashSet::new();
    let mut destroyed = Destroyed {
        structs: Vec::new(),
        symbolic: false,
    };
    while let Some((ty, whole)) = pending.pop() {
        if !seen.insert((ty.clone(), whole)) {
            continue;
        }
        match ty.kind() {
            TyKind::Struct(name, _) => {
                if whole {
                    destroyed.structs.push(ty.clone());
                }
                pending.extend(
                    fields(name)
                        .into_iter()
                        .flatten()
                        .map(|(_, field)| (field.clone(), true)),
                );
            }
            TyKind::Tuple(items) | TyKind::RuntimePack(items) | TyKind::Variant(items) => {
                pending.extend(items.iter().map(|item| (item.clone(), true)));
            }
            TyKind::ComptimeList(item) | TyKind::VariadicPack(item) => {
                pending.push((item.clone(), true));
            }
            TyKind::Infer
            | TyKind::Param { .. }
            | TyKind::Assoc { .. }
            | TyKind::Dependent(_)
            | TyKind::SelfType => destroyed.symbolic = true,
            _ => {}
        }
    }
    destroyed
}

/// The types a body's instructions tear down, each beside whether the
/// whole value dies (its destructor may run) or only its fields.
struct Walk<'f> {
    var_tys: &'f HashMap<VarId, Ty>,
    pending: Vec<(Ty, bool)>,
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
            if let MirTerm::ReturnWithCleanup { cleanup, .. }
            | MirTerm::EscapeJump { cleanup, .. } = &block.term
            {
                for &var in cleanup {
                    self.var(var, true);
                }
            }
        }
    }

    fn instruction(&mut self, instruction: &MirInstr) {
        match instruction {
            MirInstr::DropVar { var } => self.var(*var, true),
            MirInstr::ConsumeVar { var } => self.var(*var, false),
            MirInstr::Try { cleanup, .. } => {
                for &var in cleanup {
                    self.var(var, true);
                }
            }
            MirInstr::DropPlace { place }
            | MirInstr::VariantSet { place, .. }
            | MirInstr::VariantSetInitWith { place, .. } => self.place(place, true),
            MirInstr::ConsumePlace { place, .. } => self.place(place, false),
            MirInstr::PointerStorageDestroy { element, .. }
            | MirInstr::UninitStorageDestroy { element, .. } => {
                self.pending.push((element.clone(), true));
            }
            // An owned capture dies with the closure value.
            MirInstr::MakeClosure { captures, .. } => {
                for capture in captures {
                    if !matches!(capture.mode, crate::mir::MirCaptureMode::Reference) {
                        self.place(&capture.place, true);
                    }
                }
            }
            _ => {}
        }
    }

    fn var(&mut self, var: VarId, whole: bool) {
        if let Some(ty) = self.var_tys.get(&var) {
            self.pending.push((ty.clone(), whole));
        }
    }

    fn place(&mut self, place: &MirPlace, whole: bool) {
        match place
            .ty
            .as_ref()
            .or_else(|| place.projection_tys.last())
            .or_else(|| place.root_ty.as_ref().filter(|_| place.proj.is_empty()))
        {
            Some(ty) => self.pending.push((ty.clone(), whole)),
            None if place.proj.is_empty() => self.var(place.root, whole),
            None => {}
        }
    }
}

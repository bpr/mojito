//! The destructors a body runs without naming them.
//!
//! Upstream's destructor insertion leaves a call to the destructor at each
//! point a value dies, so a symbol walk reaches it. A `drop.var`, a
//! `drop.place`, a cleanup-list drop, a storage destroy, a variant store
//! (per alternative), and an owned closure capture name the destructor
//! they run on the whole value, which a symbol walk follows, and that
//! destructor's body names its fields'; the rest of what a body tears down
//! names none, and the VM and the native backend find the struct's
//! `__deinit__` from the value's type. This module answers which structs
//! those are: the types a consumed residue or a teardown naming no
//! destructor tears down whole, and every field type such a teardown may
//! reach field by field.

use crate::mir::{
    MirBlock, MirFunction, MirInstr, MirPlace, instruction_regions, variant_place_alternatives,
};
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
    /// Each type a drop tears down whole beside the destructor it names.
    pub named: Vec<(Ty, String)>,
}

/// What the body `function` destroys without naming a destructor.
///
/// That is a whole value its drops naming no destructor tear down, the fields of a value it consumes, and, through
/// `fields`, every field of a struct those reach, since a struct without a
/// destructor, or a partly moved one, drops its fields one by one.
/// [`Destroyed::named`] holds the drops naming theirs, whose fields the
/// named destructor's own body reaches.
pub fn destroyed_structs(function: &MirFunction, fields: &StructFields<'_>) -> Destroyed {
    let mut walk = Walk {
        var_tys: &function.var_tys,
        pending: Vec::new(),
        named: Vec::new(),
    };
    walk.blocks(&function.blocks);
    Destroyed {
        named: walk.named,
        ..close_over_fields(walk.pending, fields)
    }
}

/// The struct types the values `pending` reach, each beside whether the
/// whole value is reached or only its fields: a reached struct's fields
/// are reached whole through `fields`, as is each element of a tuple,
/// pack, or variant. A symbolic type marks the result symbolic.
pub(super) fn close_over_fields(
    mut pending: Vec<(Ty, bool)>,
    fields: &StructFields<'_>,
) -> Destroyed {
    let mut seen = HashSet::new();
    let mut destroyed = Destroyed {
        structs: Vec::new(),
        symbolic: false,
        named: Vec::new(),
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
            // A drop naming its destructor reaches it by symbol, and its
            // fields through that destructor's body.
            MirInstr::DropVar { var, destructor } => match destructor {
                Some(destructor) => self.named(self.var_tys.get(var), Some(destructor)),
                None => self.var(*var, true),
            },
            MirInstr::ConsumeVar { var } => self.var(*var, false),
            MirInstr::DropPlace { place, destructor } => match destructor {
                Some(destructor) => self.named(place.ty.as_ref(), Some(destructor)),
                None => self.place(place, true),
            },
            // A variant store names, per alternative, the destructor the
            // replaced payload runs under that tag; an alternative naming
            // none, or a store naming none yet, tears down by type.
            MirInstr::VariantSet {
                place, destructors, ..
            }
            | MirInstr::VariantSetInitWith {
                place, destructors, ..
            } => match variant_place_alternatives(place, self.var_tys) {
                Some(alternatives) => {
                    for (position, alternative) in alternatives.iter().enumerate() {
                        match destructors.get(position).and_then(Option::as_deref) {
                            Some(destructor) => self.named(Some(alternative), Some(destructor)),
                            None => self.pending.push((alternative.clone(), true)),
                        }
                    }
                }
                None => self.place(place, true),
            },
            MirInstr::ConsumePlace { place, .. } => self.place(place, false),
            MirInstr::PointerStorageDestroy {
                element,
                destructor,
                ..
            }
            | MirInstr::UninitStorageDestroy {
                element,
                destructor,
                ..
            } => match destructor {
                Some(destructor) => self.named(Some(element), Some(destructor)),
                None => self.pending.push((element.clone(), true)),
            },
            // An owned capture dies with the closure value, by the
            // destructor the capture names.
            MirInstr::MakeClosure { captures, .. } => {
                for capture in captures {
                    if capture.destructor_member().is_none() {
                        continue;
                    }
                    match &capture.destructor {
                        Some(destructor) => {
                            self.named(capture.place.storage_ty(self.var_tys), Some(destructor));
                        }
                        None => self.place(&capture.place, true),
                    }
                }
            }
            _ => {}
        }
    }

    /// A teardown of a value of type `ty` naming `destructor`.
    fn named(&mut self, ty: Option<&Ty>, destructor: Option<&str>) {
        if let (Some(ty), Some(destructor)) = (ty, destructor) {
            self.named.push((ty.clone(), destructor.to_owned()));
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

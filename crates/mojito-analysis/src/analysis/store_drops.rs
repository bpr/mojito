//! Store-overwrite destruction for the writes drop liveness does not
//! follow: a write into an initialized droppable place destroys the value it
//! replaces first, as Mojo's assignment does. A whole variable's
//! reassignment and a store into a part of a root the walk lays out
//! (`DropValues`) need no instruction of their own: the old value dies at
//! its last use before the redefinition. Two writes have no such definition
//! and so get a `DropPlace` right before them: a field of a root the walk
//! keeps whole (an initializer's `out self`, a `deinit` parameter,
//! `__toplevel__`'s, which has no walk), and a whole place written *through
//! a reference*, whose old value lives in the caller's storage or another
//! slot, as upstream's indirect `checkDef` destroys it before the write.
//! Each is emitted only where the move scan's replay says the replaced value
//! is initialized on every path, so every drop it places is decided, as
//! upstream's `DestructorInsertion` places it.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::{MirDeclarations, instruction_regions_mut};
use mojito_types::types::{Ty, TyKind};

/// Splice a `DropPlace` before every write in function `name` that overwrites
/// an initialized droppable place: a `Store` into a field or constant-index
/// element whose place `walked` does not define, and a whole-place write
/// through a reference handle. Parameters
/// arrive initialized and every other slot starts uninitialized, except an
/// initializer's `out self` receiver: its storage exists but each declared
/// field is uninitialized until its first store, so a constructor's first
/// store into a field destroys nothing and a second one destroys the first.
pub(super) fn elaborate_store_drops(
    f: &mut MirFunction,
    name: &str,
    declarations: &MirDeclarations,
    walked: &dyn Fn(&MirPlace) -> bool,
) {
    let entry = entry_move_states(f, initializer_receiver_entry(f, name, declarations));
    let handles = make_ref_places(&f.blocks);
    let mut overwrites: HashMap<*const MirInstr, MirPlace> = HashMap::new();
    let observed = observe_move_states(f, entry, &mut |state, instr| {
        if let Some(place) = replaced_place(instr, state, f, &handles)
            && !walked(place)
        {
            overwrites.insert(std::ptr::from_ref(instr), place.clone());
        }
    });
    if observed.is_ok() && !overwrites.is_empty() {
        splice_blocks(&mut f.blocks, &overwrites);
    }
}

/// The first store in the module-scope body `f` that refills a droppable
/// static field or element only some paths to it have moved out, as a
/// conditional-move error.
///
/// Such a store replaces a value initialized on some paths only, which no
/// decided drop can destroy, and module scope has no drop walk to destroy it
/// on the paths that hold it. Upstream rejects every executable statement at
/// file scope.
pub(super) fn module_scope_maybe_moved_refill(f: &MirFunction) -> Result<(), OwnershipError> {
    let mut refill = None;
    observe_move_states(f, entry_move_states(f, None), &mut |state, instr| {
        if let MirInstr::Store { place, src } = instr
            && refill.is_none()
            && refills_maybe_moved(state, place)
        {
            refill = Some(OwnershipError::ConditionallyMoved {
                var: place_display(&f.var_names[place.root as usize], &place_path(place)),
                span: use_span(f, *src),
            });
        }
    })?;
    refill.map_or(Ok(()), Err)
}

/// The entry state of an initializer's `out self` receiver (parameter 0 of a
/// `Type.__init__`/`__copyinit__`/`__moveinit__`): every declared field
/// uninitialized. A receiver whose struct declaration is unknown stays wholly
/// uninitialized, so none of its stores destroys anything.
pub(super) fn initializer_receiver_entry(
    f: &MirFunction,
    name: &str,
    declarations: &MirDeclarations,
) -> Option<Node> {
    let is_receiver = f.n_params > 0
        && f.var_names.first().is_some_and(|var| var == "self")
        && mojito_symbol::symbol::is_initializer_symbol(name);
    if !is_receiver {
        return None;
    }
    let fields = match f.var_tys.get(&0).map(Ty::kind) {
        Some(TyKind::Struct(struct_name, _)) => declarations.structure(struct_name).map(|decl| {
            decl.fields
                .iter()
                .map(|(field, _)| field.clone())
                .collect::<Vec<_>>()
        }),
        _ => None,
    };
    Some(fields.map_or_else(Node::moved, Node::with_uninitialized_fields))
}

/// Every variable's state entering `f`: parameters initialized, other slots
/// not, and an initializer's receiver as `receiver` has it.
pub(super) fn entry_move_states(f: &MirFunction, receiver: Option<Node>) -> Vec<Node> {
    let mut entry: Vec<Node> = (0..f.n_vars)
        .map(|var| {
            if var < f.n_params {
                Node::owned()
            } else {
                Node::moved()
            }
        })
        .collect();
    if let Some(receiver) = receiver {
        entry[0] = receiver;
    }
    entry
}

/// The place an overwriting write destroys before replacing it, if any: a
/// `Store` into a field or element, a `Store` through a place pointer or a
/// whole-variable `ref` binding, or a `WriteRef` through a `mut` parameter or
/// `mut self` receiver — whose place is the one its `MakeRef` handle names,
/// since the instruction itself carries only the register.
fn replaced_place<'a>(
    instr: &'a MirInstr,
    state: &[Node],
    f: &MirFunction,
    handles: &'a HashMap<u32, MirPlace>,
) -> Option<&'a MirPlace> {
    match instr {
        MirInstr::Store { place, .. } => (overwrites_initialized(state, place)
            || overwrites_through_reference(state, place, f))
        .then_some(place),
        MirInstr::WriteRef { reference, .. } => handles
            .get(&reference.0)
            .filter(|place| overwrites_through_reference(state, place, f)),
        _ => None,
    }
}

/// Whether a store into `place` replaces a value that must be destroyed: a
/// static field or element of droppable type whose whole subtree is
/// initialized on every path to it.
fn overwrites_initialized(state: &[Node], place: &MirPlace) -> bool {
    droppable_static_subplace(place)
        && state[place.root as usize].read(&place_path(place)).0 == Own::Owned
}

/// Whether a store into `place` refills a droppable static field or element
/// of a local root some paths to it have moved out, and others not.
fn refills_maybe_moved(state: &[Node], place: &MirPlace) -> bool {
    let path = place_path(place);
    let node = &state[place.root as usize];
    droppable_static_subplace(place)
        && place.through.is_none()
        && node.read(&path).0 == Own::MaybeMoved
        && node.base_at(&path).0 == Own::MaybeMoved
}

/// Whether `place` is a static field or element of droppable type.
fn droppable_static_subplace(place: &MirPlace) -> bool {
    let static_subplace = matches!(
        place.steps().next_back(),
        Some(Proj::Field(_) | Proj::ConstIndex(_))
    ) && !place
        .steps()
        .any(|projection| matches!(projection, Proj::UninitPayload));
    let droppable = place
        .ty
        .as_ref()
        .is_some_and(|ty| !matches!(ty.kind(), TyKind::Ref(_)) && field_needs_drop(ty));
    static_subplace && droppable
}

/// Whether a whole-place write through a reference replaces a value that must
/// be destroyed. Such a write has no redefining `DefVar` to end the old
/// value's live range: the value lives in the caller's storage (a `mut`
/// parameter or `mut self` receiver writes through its own handle) or in
/// another slot (a place pointer's pointee, a whole-variable `ref` binding).
/// Only an intact subtree qualifies — dropping a partially moved value whole
/// would free a hole, and no backend guards a drop at run time.
fn overwrites_through_reference(state: &[Node], place: &MirPlace, f: &MirFunction) -> bool {
    let Some(through) = place.through.filter(|_| place.is_whole_root()) else {
        return false;
    };
    let root = place.root as usize;
    let reference_root = through != place.root
        || (root < f.n_params && f.ref_params.get(root).copied().unwrap_or(false));
    reference_root
        && place
            .ty
            .as_ref()
            .is_some_and(|ty| !matches!(ty.kind(), TyKind::Ref(_)) && field_needs_drop(ty))
        && matches!(state[root].read(&[]).0, Own::Owned)
}

/// Every `MakeRef` handle's place, keyed by its destination register: a
/// `WriteRef` names only that register, so the place it writes is recovered
/// here. A register is defined once per function, so one map covers every
/// block, every region included.
fn make_ref_places(blocks: &[MirBlock]) -> HashMap<u32, MirPlace> {
    let mut handles = HashMap::new();
    for_each_instr_deep(blocks, &mut |instr| {
        if let MirInstr::MakeRef { dest, place } = instr {
            handles.insert(dest.0, place.clone());
        }
    });
    handles
}

/// Insert each marked write's `DropPlace` before it in `blocks`, recursing
/// into every region op's regions. A write is looked up while it is still
/// in place, before its block is rebuilt.
fn splice_blocks(blocks: &mut [MirBlock], overwrites: &HashMap<*const MirInstr, MirPlace>) {
    for block in blocks {
        let places: Vec<Option<MirPlace>> = block
            .instrs
            .iter()
            .map(|instr| overwrites.get(&std::ptr::from_ref(instr)).cloned())
            .collect();
        for instr in &mut block.instrs {
            for region in instruction_regions_mut(instr) {
                splice_blocks(region, overwrites);
            }
        }
        let instrs = std::mem::take(&mut block.instrs);
        block.instrs = instrs
            .into_iter()
            .zip(places)
            .flat_map(|(instr, place)| {
                place
                    .map(|place| MirInstr::DropPlace {
                        place,
                        destructor: None,
                    })
                    .into_iter()
                    .chain(std::iter::once(instr))
            })
            .collect();
    }
}

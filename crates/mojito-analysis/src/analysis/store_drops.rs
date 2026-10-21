//! Store-overwrite destruction: a write into an initialized droppable place
//! destroys the value it replaces first, as Mojo's assignment does. A whole
//! variable's reassignment needs no instruction of its own (the old value dies
//! at its last use before the redefining `DefVar`), but two writes have no
//! such `DefVar` and so get a `DropPlace` right before them: a field, which is
//! not a variable, and a whole place written *through a reference*, whose old
//! value lives in the caller's storage or another slot.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::{MirDeclarations, instruction_regions_mut};
use mojito_types::types::{Ty, TyKind};

/// Splice a `DropPlace` before every write in function `name` that overwrites
/// an initialized droppable place: a `Store` into a field or constant-index
/// element, and a whole-place write through a reference handle. Parameters
/// arrive initialized and every other slot starts uninitialized, except an
/// initializer's `out self` receiver: its storage exists but each declared
/// field is uninitialized until its first store, so a constructor's first
/// store into a field destroys nothing and a second one destroys the first.
pub(super) fn elaborate_store_drops(
    f: &mut MirFunction,
    name: &str,
    declarations: &MirDeclarations,
) {
    let receiver = initializer_receiver_entry(f, name, declarations);
    let uninitialized_receiver = receiver.is_some();
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
    let handles = make_ref_places(&f.blocks);
    let mut overwrites: HashMap<*const MirInstr, MirPlace> = HashMap::new();
    let observed = observe_move_states(f, entry, &mut |state, instr| {
        if let Some(place) = replaced_place(instr, state, f, &handles, uninitialized_receiver) {
            overwrites.insert(std::ptr::from_ref(instr), place.clone());
        }
    });
    if observed.is_ok() && !overwrites.is_empty() {
        splice_blocks(&mut f.blocks, &overwrites);
    }
}

/// The entry state of an initializer's `out self` receiver (parameter 0 of a
/// `Type.__init__`/`__copyinit__`/`__moveinit__`): every declared field
/// uninitialized. A receiver whose struct declaration is unknown stays wholly
/// uninitialized, so none of its stores destroys anything.
fn initializer_receiver_entry(
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
    uninitialized_receiver: bool,
) -> Option<&'a MirPlace> {
    match instr {
        MirInstr::Store { place, .. } => {
            (overwrites_initialized(state, place, uninitialized_receiver)
                || overwrites_through_reference(state, place, f))
            .then_some(place)
        }
        MirInstr::WriteRef { reference, .. } => handles
            .get(&reference.0)
            .filter(|place| overwrites_through_reference(state, place, f)),
        _ => None,
    }
}

/// Whether a store into `place` replaces a value that must be destroyed: a
/// static field or element of droppable type whose whole subtree is
/// initialized. A depth-1 place of a local root that is only maybe moved also
/// qualifies — the VM's tombstone and the native leaf flag decide at run time
/// — except below an initializer's receiver, whose leaf flags start present
/// over uninitialized storage.
fn overwrites_initialized(state: &[Node], place: &MirPlace, uninitialized_receiver: bool) -> bool {
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
    if !static_subplace || !droppable {
        return false;
    }
    let path = place_path(place);
    let node = &state[place.root as usize];
    match node.read(&path).0 {
        Own::Owned => true,
        Own::MaybeMoved => {
            path.len() == 1
                && place.through.is_none()
                && !(uninitialized_receiver && place.root == 0)
                && node.base_at(&path).0 == Own::MaybeMoved
        }
        Own::Moved => false,
    }
}

/// Whether a whole-place write through a reference replaces a value that must
/// be destroyed. Such a write has no redefining `DefVar` to end the old
/// value's live range: the value lives in the caller's storage (a `mut`
/// parameter or `mut self` receiver writes through its own handle) or in
/// another slot (a place pointer's pointee, a whole-variable `ref` binding).
/// Only an intact subtree qualifies — dropping a partially moved value whole
/// would free a hole, which the native lowering has no leaf flag to guard.
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

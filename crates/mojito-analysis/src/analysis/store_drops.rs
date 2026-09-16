//! Store-overwrite destruction: a write into an initialized droppable place
//! destroys the value it replaces first, as Mojo's assignment does. A whole
//! variable's reassignment needs no instruction of its own (the old value dies
//! at its last use before the redefining `DefVar`), but two writes have no
//! such `DefVar` and so get a `DropPlace` right before them: a field, which is
//! not a variable, and a whole place written *through a reference*, whose old
//! value lives in the caller's storage or another slot.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::MirStructDeclaration;
use mojito_types::types::Ty;

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
    structs: &[MirStructDeclaration],
) {
    let receiver = initializer_receiver_entry(f, name, structs);
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
        f.blocks = splice_blocks(&f.blocks, &overwrites);
    }
}

/// The entry state of an initializer's `out self` receiver (parameter 0 of a
/// `Type.__init__`/`__copyinit__`/`__moveinit__`): every declared field
/// uninitialized. A receiver whose struct declaration is unknown stays wholly
/// uninitialized, so none of its stores destroys anything.
fn initializer_receiver_entry(
    f: &MirFunction,
    name: &str,
    structs: &[MirStructDeclaration],
) -> Option<Node> {
    let is_receiver = f.n_params > 0
        && f.var_names.first().is_some_and(|var| var == "self")
        && mojito_symbol::symbol::is_initializer_symbol(name);
    if !is_receiver {
        return None;
    }
    let fields = match f.var_tys.get(&0) {
        Some(Ty::Struct(struct_name, _)) => structs
            .iter()
            .find(|decl| &decl.name == struct_name)
            .map(|decl| {
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
        place.proj.last(),
        Some(Proj::Field(_) | Proj::ConstIndex(_))
    ) && !place
        .proj
        .iter()
        .any(|projection| matches!(projection, Proj::UninitPayload));
    let droppable = place
        .ty
        .as_ref()
        .is_some_and(|ty| !matches!(ty, Ty::Ref(_)) && field_needs_drop(ty));
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
    let Some(through) = place.through.filter(|_| place.proj.is_empty()) else {
        return false;
    };
    let root = place.root as usize;
    let reference_root = through != place.root
        || (root < f.n_params && f.ref_params.get(root).copied().unwrap_or(false));
    reference_root
        && place
            .ty
            .as_ref()
            .is_some_and(|ty| !matches!(ty, Ty::Ref(_)) && field_needs_drop(ty))
        && matches!(state[root].read(&[]).0, Own::Owned)
}

/// Every `MakeRef` handle's place, keyed by its destination register: a
/// `WriteRef` names only that register, so the place it writes is recovered
/// here. A register is defined once per function, so one map covers every
/// block, `try` regions included.
fn make_ref_places(blocks: &[MirBlock]) -> HashMap<u32, MirPlace> {
    let mut handles = HashMap::new();
    collect_make_refs(blocks, &mut handles);
    handles
}

fn collect_make_refs(blocks: &[MirBlock], handles: &mut HashMap<u32, MirPlace>) {
    for block in blocks {
        for instr in &block.instrs {
            match instr {
                MirInstr::MakeRef { dest, place } => {
                    handles.insert(dest.0, place.clone());
                }
                MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } => {
                    collect_make_refs(body, handles);
                    if let Some((_, blocks)) = handler.as_ref() {
                        collect_make_refs(blocks, handles);
                    }
                    for blocks in orelse.as_deref().into_iter().chain(finalbody.as_deref()) {
                        collect_make_refs(blocks, handles);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Rebuild `blocks`, inserting each marked write's `DropPlace` before it,
/// recursing into `try` regions.
fn splice_blocks(
    blocks: &[MirBlock],
    overwrites: &HashMap<*const MirInstr, MirPlace>,
) -> Vec<MirBlock> {
    blocks
        .iter()
        .map(|block| {
            let mut instrs = Vec::with_capacity(block.instrs.len());
            for instr in &block.instrs {
                if let Some(place) = overwrites.get(&std::ptr::from_ref(instr)) {
                    instrs.push(MirInstr::DropPlace {
                        place: place.clone(),
                    });
                }
                instrs.push(splice_instruction(instr, overwrites));
            }
            MirBlock {
                instrs,
                term: block.term.clone(),
            }
        })
        .collect()
}

fn splice_instruction(
    instr: &MirInstr,
    overwrites: &HashMap<*const MirInstr, MirPlace>,
) -> MirInstr {
    match instr {
        MirInstr::Try {
            body,
            handler,
            orelse,
            finalbody,
            cleanup,
        } => MirInstr::Try {
            body: splice_blocks(body, overwrites),
            handler: handler
                .as_ref()
                .map(|(binding, blocks)| (*binding, splice_blocks(blocks, overwrites))),
            orelse: orelse
                .as_deref()
                .map(|blocks| splice_blocks(blocks, overwrites)),
            finalbody: finalbody
                .as_deref()
                .map(|blocks| splice_blocks(blocks, overwrites)),
            cleanup: cleanup.clone(),
        },
        other => other.clone(),
    }
}

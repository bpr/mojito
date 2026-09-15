//! Store-overwrite destruction: a `Store` into an initialized droppable
//! sub-place destroys the value it replaces first, as Mojo's assignment does.
//! A whole-variable reassignment needs no instruction of its own (the old
//! value dies at its last use before the redefining `DefVar`), but a field
//! is not a variable, so its old value gets a `DropPlace` right before the
//! write.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::MirStructDeclaration;
use mojito_types::types::Ty;

/// Splice a `DropPlace` before every `Store` that overwrites an initialized
/// droppable field or constant-index element of function `name`. Parameters
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
    let mut overwrites: HashSet<*const MirInstr> = HashSet::new();
    let observed = observe_move_states(f, entry, &mut |state, instr| {
        if let MirInstr::Store { place, .. } = instr
            && overwrites_initialized(state, place, uninitialized_receiver)
        {
            overwrites.insert(std::ptr::from_ref(instr));
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

/// Rebuild `blocks`, inserting a `DropPlace` before each marked `Store`,
/// recursing into `try` regions.
fn splice_blocks(blocks: &[MirBlock], overwrites: &HashSet<*const MirInstr>) -> Vec<MirBlock> {
    blocks
        .iter()
        .map(|block| {
            let mut instrs = Vec::with_capacity(block.instrs.len());
            for instr in &block.instrs {
                if let MirInstr::Store { place, .. } = instr
                    && overwrites.contains(&std::ptr::from_ref(instr))
                {
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

fn splice_instruction(instr: &MirInstr, overwrites: &HashSet<*const MirInstr>) -> MirInstr {
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

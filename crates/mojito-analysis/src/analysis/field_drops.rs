//! Per-field last-use destruction for `deinit` parameters: after the
//! variable-granular drop elaboration, each direct field of a `deinit
//! self`/`deinit existing` receiver dies at that field's own last use (an
//! unused one at entry), as Mojo destroys a destructor body's fields. The
//! receiver's `ConsumeVar` teardown stays where the variable pass put it and
//! destroys only the fields that survive to that point.

use super::*;
use mojito_mir::mir::MirStructDeclaration;
use mojito_mir::mir::verify::{
    instruction_operand_regs, instruction_places, instruction_result_regs,
};
use mojito_types::types::Ty;

/// Splice `DropPlace`s for every refinable field of every `deinit`
/// parameter of struct type with a known declaration.
pub(super) fn refine_deinit_fields(f: &mut MirFunction, structs: &[MirStructDeclaration]) {
    let roots: Vec<DeinitRoot> = (0..f.n_params)
        .filter(|&i| f.deinit_params.get(i).copied().unwrap_or(false))
        .filter_map(|i| deinit_root(f, i as VarId, structs))
        .collect();
    for root in roots {
        let top_live_in = top_level_field_liveness(&f.blocks, &root);
        let seeds = FieldSeeds {
            fall_off: FieldSet::new(),
            raise: FieldSet::new(),
            finally_live: FieldSet::new(),
            top_live_in: &top_live_in,
        };
        let mut blocks = std::mem::take(&mut f.blocks);
        refine_region(&mut blocks, &root, &seeds, Some(&root.all));
        f.blocks = blocks;
    }
}

/// Declaration positions of a receiver's refinable fields.
type FieldSet = BTreeSet<usize>;

struct DeinitRoot {
    var: VarId,
    root_ty: Ty,
    /// Every field in declaration order (name, type).
    fields: Vec<(String, Ty)>,
    /// The positions the pass may destroy individually: droppable, and never
    /// borrowed or moved below depth one anywhere in the body.
    all: FieldSet,
}

impl DeinitRoot {
    fn position(&self, name: &str) -> Option<usize> {
        self.fields
            .iter()
            .position(|(field, _)| field == name)
            .filter(|position| self.all.contains(position))
    }

    fn place(&self, position: usize) -> MirPlace {
        let (name, ty) = &self.fields[position];
        let mut place = MirPlace::root(self.var, Some(self.root_ty.clone()));
        place.project(Proj::Field(name.clone()), ty.clone());
        place
    }
}

/// Live-out seeds for one region, per exit kind (the field analogue of
/// [`RegionSeeds`](super::drops::RegionSeeds)).
struct FieldSeeds<'a> {
    fall_off: FieldSet,
    raise: FieldSet,
    finally_live: FieldSet,
    top_live_in: &'a [FieldSet],
}

fn deinit_root(
    f: &MirFunction,
    var: VarId,
    structs: &[MirStructDeclaration],
) -> Option<DeinitRoot> {
    let root_ty = f.var_tys.get(&var)?.clone();
    let Ty::Struct(name, _) = &root_ty else {
        return None;
    };
    let decl = structs.iter().find(|decl| &decl.name == name)?;
    let mut pinned: HashSet<String> = HashSet::new();
    for_each_instr_deep(&f.blocks, &mut |instr| {
        let borrowing = matches!(
            instr,
            MirInstr::MakeRef { .. }
                | MirInstr::EstablishLoans { .. }
                | MirInstr::MakeClosure { .. }
        );
        for place in instruction_places(instr) {
            if place.root != var {
                continue;
            }
            let Some(Proj::Field(field)) = place.proj.first() else {
                continue;
            };
            let deep_move = place.proj.len() > 1
                && matches!(
                    instr,
                    MirInstr::MovePlace { .. } | MirInstr::ConsumePlace { .. }
                );
            if borrowing || deep_move || place.through.is_some() {
                pinned.insert(field.clone());
            }
        }
    });
    let all: FieldSet = decl
        .fields
        .iter()
        .enumerate()
        .filter(|(_, (name, ty))| field_needs_drop(ty) && !pinned.contains(name))
        .map(|(position, _)| position)
        .collect();
    if all.is_empty() {
        return None;
    }
    Some(DeinitRoot {
        var,
        root_ty,
        fields: decl.fields.clone(),
        all,
    })
}

/// Whether destroying a field of this type can do observable work (run a
/// destructor, release storage). Scalars, pointers, and references die
/// silently, so they need no instruction.
fn field_needs_drop(ty: &Ty) -> bool {
    may_alias_owned_storage(ty) || matches!(ty, Ty::Func { .. } | Ty::Error)
}

/// The top-level per-block field live-in sets, bounding `EscapeJump` targets.
fn top_level_field_liveness(blocks: &[MirBlock], root: &DeinitRoot) -> Vec<FieldSet> {
    let empty: Vec<FieldSet> = vec![FieldSet::new(); blocks.len()];
    let seeds = FieldSeeds {
        fall_off: FieldSet::new(),
        raise: FieldSet::new(),
        finally_live: FieldSet::new(),
        top_live_in: &empty,
    };
    region_field_liveness(blocks, root, &seeds).0
}

/// Elaborate one region (a function body or a `try` mini-CFG) in place:
/// in-block last-use drops, edge drops, entry drops (when `entry_present`
/// names the fields alive on entry), recursing into nested `try`s. Returns
/// the region's entry liveness.
fn refine_region(
    blocks: &mut Vec<MirBlock>,
    root: &DeinitRoot,
    seeds: &FieldSeeds,
    entry_present: Option<&FieldSet>,
) -> FieldSet {
    let nb = blocks.len();
    if nb == 0 {
        return seeds.fall_off.clone();
    }
    let (live_in, uses) = region_field_liveness(blocks, root, seeds);

    // (1) Block-internal deaths.
    for b in 0..nb {
        let live_out = block_field_live_out(blocks, b, &live_in, seeds, root);
        let instrs = std::mem::take(&mut blocks[b].instrs);
        let n = instrs.len();
        let mut live_after = vec![FieldSet::new(); n];
        let mut live_before = vec![FieldSet::new(); n];
        let mut live = live_out;
        for i in (0..n).rev() {
            live_after[i] = live.clone();
            live.extend(uses[b][i].iter().copied());
            if may_raise(&instrs[i]) {
                live.extend(seeds.raise.iter().copied());
            }
            live_before[i] = live.clone();
        }

        let mut rebuilt = Vec::with_capacity(n);
        let mut i = 0;
        while i < n {
            let mut instr = instrs[i].clone();
            if let MirInstr::Try { .. } = &mut instr {
                refine_try(&mut instr, root, &live_after[i], seeds);
            }
            let moved = moved_fields(&instrs[i], root);
            let dying: Vec<usize> = live_before[i]
                .difference(&live_after[i])
                .copied()
                .filter(|position| !moved.contains(position))
                .collect();
            rebuilt.push(instr);
            i += 1;
            // Keep the variable pass's trailing drop group together; a
            // `ConsumeVar` of the receiver in it already destroys these
            // fields at this point.
            let mut consumed = false;
            while i < n && is_drop_instr(&instrs[i]) {
                consumed |= matches!(&instrs[i], MirInstr::ConsumeVar { var } if *var == root.var);
                rebuilt.push(instrs[i].clone());
                i += 1;
            }
            if !consumed {
                append_field_drops(&mut rebuilt, root, dying);
            }
        }
        blocks[b].instrs = rebuilt;
    }

    // (1b) Entry deaths: fields present on entry that no path reads.
    if let Some(present) = entry_present {
        let dead: Vec<usize> = present.difference(&live_in[0]).copied().collect();
        prepend_field_drops(&mut blocks[0].instrs, root, dead);
    }

    // (2) Edge deaths.
    let mut pred_count = vec![0usize; nb];
    for block in blocks.iter() {
        for s in successors(&block.term) {
            if s < nb {
                pred_count[s] += 1;
            }
        }
    }
    for p in 0..nb {
        let mut succs: Vec<usize> = successors(&blocks[p].term)
            .into_iter()
            .filter(|s| *s < nb)
            .collect();
        succs.sort_unstable();
        succs.dedup();
        let n_succ = succs.len();
        let live_out_p: FieldSet = succs
            .iter()
            .flat_map(|s| live_in[*s].iter().copied())
            .collect();
        for &s in &succs {
            let dying: Vec<usize> = live_out_p
                .iter()
                .copied()
                .filter(|position| !live_in[s].contains(position))
                .collect();
            if dying.is_empty() {
                continue;
            }
            if n_succ == 1 {
                append_field_drops(&mut blocks[p].instrs, root, dying);
            } else if pred_count[s] == 1 {
                prepend_field_drops(&mut blocks[s].instrs, root, dying);
            } else {
                let new_idx = blocks.len();
                let mut instrs = Vec::new();
                append_field_drops(&mut instrs, root, dying);
                blocks.push(MirBlock {
                    instrs,
                    term: MirTerm::Jump(s),
                });
                rewire_target(&mut blocks[p].term, s, new_idx);
            }
        }
    }

    live_in[0].clone()
}

/// Elaborate the four regions of one `try` in place, seeded like
/// [`elaborate_try_interior`](super::drops::elaborate_try_interior):
/// `finally` first, then handler/`else`, then the body.
fn refine_try(try_instr: &mut MirInstr, root: &DeinitRoot, after: &FieldSet, seeds: &FieldSeeds) {
    let MirInstr::Try {
        body,
        handler,
        orelse,
        finalbody,
        ..
    } = try_instr
    else {
        return;
    };
    let fin_live = match finalbody {
        Some(fb) => {
            let fin_seeds = FieldSeeds {
                fall_off: after.clone(),
                raise: seeds.raise.clone(),
                finally_live: after.clone(),
                top_live_in: seeds.top_live_in,
            };
            refine_region(fb, root, &fin_seeds, None)
        }
        None => after.clone(),
    };
    let mut outward_raise = fin_live.clone();
    outward_raise.extend(seeds.raise.iter().copied());

    let inner_seeds = FieldSeeds {
        fall_off: fin_live.clone(),
        raise: outward_raise.clone(),
        finally_live: fin_live.clone(),
        top_live_in: seeds.top_live_in,
    };
    // The handler's entry liveness seeds the body's raise edges; the fields
    // alive at any of the body's raise points are present on handler entry.
    let handler_entry = handler.as_ref().map(|(_, h)| {
        region_field_liveness(h, root, &inner_seeds)
            .0
            .first()
            .cloned()
            .unwrap_or_default()
    });
    let orelse_live = orelse
        .as_mut()
        .map(|e| refine_region(e, root, &inner_seeds, None));
    let body_seeds = FieldSeeds {
        fall_off: orelse_live.unwrap_or_else(|| fin_live.clone()),
        raise: handler_entry
            .clone()
            .unwrap_or_else(|| outward_raise.clone()),
        finally_live: fin_live.clone(),
        top_live_in: seeds.top_live_in,
    };
    let raise_present = raise_point_liveness(body, root, &body_seeds);
    if let Some((_, h)) = handler.as_mut() {
        refine_region(h, root, &inner_seeds, Some(&raise_present));
    }
    refine_region(body, root, &body_seeds, None);
}

/// Backward field liveness over a region's blocks: per-block live-in sets and
/// the per-instruction field uses (direct place uses plus register-carried
/// provenance), recursing into nested `try`s without mutating them.
fn region_field_liveness(
    blocks: &[MirBlock],
    root: &DeinitRoot,
    seeds: &FieldSeeds,
) -> (Vec<FieldSet>, Vec<Vec<FieldSet>>) {
    let nb = blocks.len();
    let carried = register_field_provenance(blocks, root);
    let mut uses: Vec<Vec<FieldSet>> = blocks
        .iter()
        .enumerate()
        .map(|(b, block)| {
            block
                .instrs
                .iter()
                .enumerate()
                .map(|(i, instr)| {
                    let mut set = direct_field_uses(instr, root);
                    set.extend(carried[b][i].iter().copied());
                    set
                })
                .collect()
        })
        .collect();
    let mut live_in: Vec<FieldSet> = vec![FieldSet::new(); nb];
    let mut changed = true;
    while changed {
        changed = false;
        for b in (0..nb).rev() {
            let mut live = block_field_live_out(blocks, b, &live_in, seeds, root);
            for (i, instr) in blocks[b].instrs.iter().enumerate().rev() {
                if let MirInstr::Try { .. } = instr {
                    let entry = try_entry_liveness(instr, root, &live, seeds);
                    uses[b][i] = entry.clone();
                    live = entry;
                } else {
                    live.extend(uses[b][i].iter().copied());
                    if may_raise(instr) {
                        live.extend(seeds.raise.iter().copied());
                    }
                }
            }
            if live != live_in[b] {
                live_in[b] = live;
                changed = true;
            }
        }
    }
    (live_in, uses)
}

/// The fields alive at any potentially-raising instruction of a region —
/// what its handler may still find on entry.
fn raise_point_liveness(blocks: &[MirBlock], root: &DeinitRoot, seeds: &FieldSeeds) -> FieldSet {
    let (live_in, uses) = region_field_liveness(blocks, root, seeds);
    let mut present = FieldSet::new();
    for b in 0..blocks.len() {
        let mut live = block_field_live_out(blocks, b, &live_in, seeds, root);
        for (i, instr) in blocks[b].instrs.iter().enumerate().rev() {
            if let MirInstr::Try { .. } = instr {
                live = try_entry_liveness(instr, root, &live, seeds);
                present.extend(live.iter().copied());
            } else {
                live.extend(uses[b][i].iter().copied());
                if may_raise(instr) {
                    live.extend(seeds.raise.iter().copied());
                    present.extend(live.iter().copied());
                }
            }
        }
    }
    present
}

/// A nested `try`'s entry liveness (pure), seeded like [`refine_try`].
fn try_entry_liveness(
    try_instr: &MirInstr,
    root: &DeinitRoot,
    after: &FieldSet,
    seeds: &FieldSeeds,
) -> FieldSet {
    let MirInstr::Try {
        body,
        handler,
        orelse,
        finalbody,
        ..
    } = try_instr
    else {
        return after.clone();
    };
    let region_entry = |blocks: &[MirBlock], seeds: &FieldSeeds| -> FieldSet {
        if blocks.is_empty() {
            seeds.fall_off.clone()
        } else {
            region_field_liveness(blocks, root, seeds).0[0].clone()
        }
    };
    let fin_live = match finalbody {
        Some(fb) => region_entry(
            fb,
            &FieldSeeds {
                fall_off: after.clone(),
                raise: seeds.raise.clone(),
                finally_live: after.clone(),
                top_live_in: seeds.top_live_in,
            },
        ),
        None => after.clone(),
    };
    let mut outward_raise = fin_live.clone();
    outward_raise.extend(seeds.raise.iter().copied());
    let inner_seeds = FieldSeeds {
        fall_off: fin_live.clone(),
        raise: outward_raise.clone(),
        finally_live: fin_live.clone(),
        top_live_in: seeds.top_live_in,
    };
    let handler_entry = handler.as_ref().map(|(_, h)| region_entry(h, &inner_seeds));
    let orelse_entry = orelse.as_ref().map(|e| region_entry(e, &inner_seeds));
    let body_seeds = FieldSeeds {
        fall_off: orelse_entry.unwrap_or_else(|| fin_live.clone()),
        raise: handler_entry.unwrap_or(outward_raise),
        finally_live: fin_live,
        top_live_in: seeds.top_live_in,
    };
    region_entry(body, &body_seeds)
}

/// A region block's live-out per terminator kind (the field analogue of
/// [`region_block_live_out`](super::drops::region_block_live_out)); a
/// cleanup list naming the receiver keeps every field alive to the exit.
fn block_field_live_out(
    blocks: &[MirBlock],
    b: usize,
    live_in: &[FieldSet],
    seeds: &FieldSeeds,
    root: &DeinitRoot,
) -> FieldSet {
    match &blocks[b].term {
        MirTerm::Jump(_) | MirTerm::Branch { .. } => {
            let mut out = FieldSet::new();
            for s in successors(&blocks[b].term) {
                if let Some(live) = live_in.get(s) {
                    out.extend(live.iter().copied());
                }
            }
            out
        }
        MirTerm::FallOff => seeds.fall_off.clone(),
        MirTerm::Return(_) => seeds.finally_live.clone(),
        MirTerm::ReturnWithCleanup { cleanup, .. } => {
            let mut out = seeds.finally_live.clone();
            if cleanup.contains(&root.var) {
                out.extend(root.all.iter().copied());
            }
            out
        }
        MirTerm::EscapeJump { target, cleanup } => {
            let mut out = seeds.finally_live.clone();
            if cleanup.contains(&root.var) {
                out.extend(root.all.iter().copied());
            }
            if let Some(live) = seeds.top_live_in.get(*target) {
                out.extend(live.iter().copied());
            }
            out
        }
    }
}

/// The receiver fields an instruction reads through its places: a depth-one
/// field projection names that field; a bare receiver touch (a whole-variable
/// use, an unprojected place, a `through` reference) touches every field.
fn direct_field_uses(instr: &MirInstr, root: &DeinitRoot) -> FieldSet {
    let mut set = FieldSet::new();
    let mut whole = false;
    match instr {
        MirInstr::UseVar { var, .. } | MirInstr::KeepAlive { var } if *var == root.var => {
            whole = true;
        }
        MirInstr::HasNext { iter, .. }
        | MirInstr::Next { iter, .. }
        | MirInstr::TryNext { iter, .. }
            if *iter == root.var =>
        {
            whole = true;
        }
        _ => {}
    }
    for place in instruction_places(instr) {
        if place.through == Some(root.var) {
            whole = true;
        }
        if place.root != root.var {
            continue;
        }
        match place.proj.first() {
            Some(Proj::Field(field)) => {
                if let Some(position) = root.position(field) {
                    set.insert(position);
                }
            }
            _ => whole = true,
        }
    }
    if whole {
        set.extend(root.all.iter().copied());
    }
    set
}

/// The receiver fields an instruction transfers out (its new owner drops
/// them, so no `DropPlace` follows).
fn moved_fields(instr: &MirInstr, root: &DeinitRoot) -> FieldSet {
    let mut set = FieldSet::new();
    if let MirInstr::MovePlace { place, .. } | MirInstr::ConsumePlace { place, .. } = instr
        && place.root == root.var
        && let Some(Proj::Field(field)) = place.proj.first()
        && let Some(position) = root.position(field)
    {
        set.insert(position);
    }
    if let MirInstr::MakeClosure { captures, .. } = instr {
        for capture in captures {
            if capture.mode == MirCaptureMode::Move
                && capture.place.root == root.var
                && let Some(Proj::Field(field)) = capture.place.proj.first()
                && let Some(position) = root.position(field)
            {
                set.insert(position);
            }
        }
    }
    set
}

/// Per-instruction fields kept alive by the registers an instruction reads:
/// a load or handle of a receiver field carries that field to every consumer
/// (results of non-call instructions inherit it; a call result is a fresh
/// value; a scalar load borrows for one hop only), mirroring
/// [`transfer_register_loans`](super::register_loans::transfer_register_loans).
fn register_field_provenance(blocks: &[MirBlock], root: &DeinitRoot) -> Vec<Vec<FieldSet>> {
    type Carry = BTreeMap<u32, (FieldSet, FieldSet)>;
    fn join(mut left: Carry, right: &Carry) -> Carry {
        for (register, (carry, single)) in right {
            let entry = left.entry(*register).or_default();
            entry.0.extend(carry.iter().copied());
            entry.1.extend(single.iter().copied());
        }
        left
    }
    fn transfer(state: &mut Carry, instr: &MirInstr, root: &DeinitRoot) -> FieldSet {
        let mut operands = Vec::new();
        instruction_operand_regs(instr, &mut operands);
        let mut carried = FieldSet::new();
        let mut uses = FieldSet::new();
        for operand in operands {
            if let Some((carry, single)) = state.get(&operand.0) {
                carried.extend(carry.iter().copied());
                uses.extend(carry.iter().copied());
                uses.extend(single.iter().copied());
            }
        }
        let mut single_hop = FieldSet::new();
        if let MirInstr::MakeRef { place, .. } | MirInstr::LoadPlace { place, .. } = instr
            && place.root == root.var
            && let Some(Proj::Field(field)) = place.proj.first()
            && let Some(position) = root.position(field)
        {
            let scalar = matches!(instr, MirInstr::LoadPlace { .. })
                && place
                    .ty
                    .as_ref()
                    .is_some_and(|ty| !may_alias_owned_storage(ty));
            if scalar {
                single_hop.insert(position);
            } else {
                carried.insert(position);
            }
        }
        let call_result = matches!(
            instr,
            MirInstr::Call { .. }
                | MirInstr::CallIndirect { .. }
                | MirInstr::MethodCall { .. }
                | MirInstr::Index { .. }
                | MirInstr::Slice { .. }
                | MirInstr::MultiIndex { .. }
                | MirInstr::BinOp {
                    resolved: Some(_),
                    ..
                }
        );
        let mut results = Vec::new();
        instruction_result_regs(instr, &mut results);
        for result in results {
            if call_result || (carried.is_empty() && single_hop.is_empty()) {
                state.remove(&result.0);
            } else {
                state.insert(result.0, (carried.clone(), single_hop.clone()));
            }
        }
        uses
    }

    let nb = blocks.len();
    let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); nb];
    for (b, block) in blocks.iter().enumerate() {
        for s in successors(&block.term) {
            if s < nb {
                predecessors[s].push(b);
            }
        }
    }
    let mut incoming: Vec<Option<Carry>> = vec![None; nb];
    let mut outgoing: Vec<Option<Carry>> = vec![None; nb];
    let mut changed = true;
    while changed {
        changed = false;
        for b in 0..nb {
            let new_in = if b == 0 || predecessors[b].is_empty() {
                Carry::new()
            } else {
                let mut states = predecessors[b].iter().filter_map(|p| outgoing[*p].as_ref());
                let Some(first) = states.next() else {
                    continue;
                };
                states.fold(first.clone(), join)
            };
            let mut new_out = new_in.clone();
            for instr in &blocks[b].instrs {
                transfer(&mut new_out, instr, root);
            }
            if incoming[b].as_ref() != Some(&new_in) || outgoing[b].as_ref() != Some(&new_out) {
                incoming[b] = Some(new_in);
                outgoing[b] = Some(new_out);
                changed = true;
            }
        }
    }
    blocks
        .iter()
        .enumerate()
        .map(|(b, block)| {
            let mut state = incoming[b].clone().unwrap_or_default();
            block
                .instrs
                .iter()
                .map(|instr| transfer(&mut state, instr, root))
                .collect()
        })
        .collect()
}

fn is_drop_instr(instr: &MirInstr) -> bool {
    matches!(
        instr,
        MirInstr::DropVar { .. } | MirInstr::ConsumeVar { .. } | MirInstr::DropPlace { .. }
    )
}

/// Append `DropPlace`s in declaration order.
fn append_field_drops(instrs: &mut Vec<MirInstr>, root: &DeinitRoot, mut positions: Vec<usize>) {
    positions.sort_unstable();
    positions.dedup();
    for position in positions {
        instrs.push(MirInstr::DropPlace {
            place: root.place(position),
        });
    }
}

/// Prepend `DropPlace`s (declaration order) after any leading drop group,
/// unless that group already consumes the receiver.
fn prepend_field_drops(instrs: &mut Vec<MirInstr>, root: &DeinitRoot, mut positions: Vec<usize>) {
    positions.sort_unstable();
    positions.dedup();
    let lead = instrs
        .iter()
        .take_while(|instr| is_drop_instr(instr))
        .count();
    if instrs[..lead]
        .iter()
        .any(|instr| matches!(instr, MirInstr::ConsumeVar { var } if *var == root.var))
    {
        return;
    }
    for (offset, position) in positions.into_iter().enumerate() {
        instrs.insert(
            lead + offset,
            MirInstr::DropPlace {
                place: root.place(position),
            },
        );
    }
}

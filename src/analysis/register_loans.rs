//! Register-loan dataflow: which registers carry loans of owned
//! storage across instructions.

use super::*;

#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct RegisterLoanState {
    owners: BTreeMap<u32, BTreeSet<VarId>>,
}

pub(super) fn join_register_loan_states(
    mut left: RegisterLoanState,
    right: &RegisterLoanState,
) -> RegisterLoanState {
    for (register, owners) in &right.owners {
        left.owners.entry(*register).or_default().extend(owners);
    }
    left
}

pub(super) fn active_register_loan_roots(
    references: impl IntoIterator<Item = VarId>,
    state: &LoanGenerationState,
    generations: &BTreeMap<u32, DropLoanGeneration>,
) -> BTreeSet<VarId> {
    let mut roots = BTreeSet::new();
    let mut pending: Vec<VarId> = references.into_iter().collect();
    let mut visited = BTreeSet::new();
    while let Some(reference) = pending.pop() {
        if !visited.insert(reference) {
            continue;
        }
        let Some(active) = state.active.get(&reference) else {
            continue;
        };
        for generation in active {
            let Some(generation) = generations.get(generation) else {
                continue;
            };
            if !generation.propagate_through_registers {
                continue;
            }
            for root in &generation.roots {
                if roots.insert(*root) {
                    pending.push(*root);
                }
            }
        }
    }
    roots
}

/// Transfer transient owner provenance through one SSA instruction and return
/// the roots the instruction consumes. The returned roots become ordinary drop
/// liveness uses at this exact program point. Results inherit operand provenance
/// until a terminal instruction such as `DefVar` consumes them, allowing
/// `MakeRef -> ReadRef -> Call` to retain an owner through the outer call without
/// extending the source reference's generation to function exit.
pub(super) fn transfer_register_loans(
    registers: &mut RegisterLoanState,
    generations: &LoanGenerationState,
    instruction: &MirInstr,
    loan_roots: &BTreeMap<u32, DropLoanGeneration>,
    register_types: &HashMap<u32, crate::types::Ty>,
) -> Vec<VarId> {
    let mut owners = BTreeSet::new();
    let mut operands = Vec::new();
    crate::mir::verify::instruction_operand_regs(instruction, &mut operands);
    for operand in operands {
        if let Some(provenance) = registers.owners.get(&operand.0) {
            owners.extend(provenance);
        }
    }

    owners.extend(active_register_loan_roots(
        var_uses(instruction).into_iter().map(|(var, _)| var),
        generations,
        loan_roots,
    ));

    // `MakeRef` creates a transient handle directly into its place root.  That
    // root is not necessarily an established reference generation itself (for
    // example `box.value` where `box` merely *contains* a stored reference), so
    // the generation closure above cannot discover it.  Retain the storage root
    // through every SSA consumer of the handle; `through` additionally keeps a
    // substituted reference binding alive when the place was reached via one.
    //
    // `LoadPlace` needs the same retention for a different reason: it reads the
    // place shallowly, so a pointer-owning (lifecycle) result register aliases
    // the root's storage until a `CopyValue` runs the copy lifecycle or a
    // consuming call finishes. Dropping the root between the load and that
    // consumer would free storage the pending register still references. A
    // scalar read owns its value outright and keeps the pre-existing ASAP
    // destruction point; only an aggregate (or unknown-typed) result retains.
    let retained_place = match instruction {
        MirInstr::MakeRef { place, .. } => Some(place),
        MirInstr::LoadPlace { dest, place } => place
            .ty
            .as_ref()
            .or_else(|| register_types.get(&dest.0))
            .is_none_or(may_alias_owned_storage)
            .then_some(place),
        _ => None,
    };
    if let Some(place) = retained_place {
        owners.insert(place.root);
        if let Some(reference) = place.through {
            owners.insert(reference);
        }
    }

    // A nominal subscript returning `ref T` establishes a transient handle to
    // its retained receiver place. Unlike `MakeRef`, the handle is produced by
    // the callee, so no explicit loan generation exists at this instruction.
    // Seed the destination register with the receiver root directly; subsequent
    // `ReadRef`/call uses then keep that caller slot alive until the handle has
    // actually been consumed.
    let reference_subscript_place = match instruction {
        MirInstr::Index {
            dest,
            base_place: Some(place),
            ..
        }
        | MirInstr::Slice {
            dest,
            object_place: Some(place),
            ..
        }
        | MirInstr::MultiIndex {
            dest,
            object_place: Some(place),
            ..
        } if matches!(register_types.get(&dest.0), Some(crate::types::Ty::Ref(_))) => Some(place),
        // A direct method call returning `ref T` (no adapter) is the same
        // callee-produced transient handle as a reference subscript: the
        // result register borrows the receiver's storage, which must outlive
        // every consumer of the handle — not just the call instruction.
        MirInstr::MethodCall {
            reference_result: Some(_),
            result_adapter: None,
            recv_place: Some(place),
            ..
        } => Some(place),
        _ => None,
    };
    let mut reference_seeded = BTreeSet::new();
    if let Some(place) = reference_subscript_place {
        reference_seeded.insert(place.root);
        if let Some(reference) = place.through {
            reference_seeded.insert(reference);
        }
    }
    owners.extend(reference_seeded.iter().copied());

    // A call-family result is a fresh independent value — a return runs the
    // copy/move lifecycle out of the callee frame — so operand provenance is
    // consumed at the call (the roots are uses at this exact point) and does
    // not flow into the result. Only the explicit reference-result seeding
    // above (a callee-produced handle into caller storage) carries through.
    let call_result = matches!(
        instruction,
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
    let result_owners = if call_result {
        &reference_seeded
    } else {
        &owners
    };
    let mut results = Vec::new();
    crate::mir::verify::instruction_result_regs(instruction, &mut results);
    for result in results {
        if result_owners.is_empty() {
            registers.owners.remove(&result.0);
        } else {
            registers.owners.insert(result.0, result_owners.clone());
        }
    }
    owners.into_iter().collect()
}

/// Whether a shallowly read value of this type can alias heap storage owned by
/// its source place — an aggregate whose fields may hold owning pointers (the
/// self-hosted `String`/`List` shape). Scalars and compile-time values own
/// their bits; a bare `Pointer` read aliases by design (the `unsafe_*`
/// vocabulary makes its lifetime the user's obligation).
pub(super) fn may_alias_owned_storage(ty: &crate::types::Ty) -> bool {
    use crate::types::Ty;
    matches!(
        ty,
        Ty::Struct(..)
            | Ty::Tuple(_)
            | Ty::RuntimePack(_)
            | Ty::Variant(_)
            | Ty::ComptimeList(_)
            | Ty::Param { .. }
            | Ty::Assoc { .. }
    )
}

/// Per-instruction transient owner uses and per-block incoming register-loan
/// states over an arbitrary block vector (a function body or a `try` region
/// mini-CFG) with an explicit entry state.
pub(super) fn register_loan_uses_over(
    blocks: &[MirBlock],
    generation_entries: &[LoanGenerationState],
    entry_registers: &RegisterLoanState,
    loan_roots: &BTreeMap<u32, DropLoanGeneration>,
    generation_dests: &BTreeMap<u32, Option<MirInteriorOrigin>>,
    reg_types: &HashMap<u32, crate::types::Ty>,
) -> (Vec<Vec<Vec<VarId>>>, Vec<RegisterLoanState>) {
    let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); blocks.len()];
    for (block, body) in blocks.iter().enumerate() {
        for successor in successors(&body.term) {
            if successor < blocks.len() {
                predecessors[successor].push(block);
            }
        }
    }

    let mut incoming: Vec<Option<RegisterLoanState>> = vec![None; blocks.len()];
    let mut outgoing: Vec<Option<RegisterLoanState>> = vec![None; blocks.len()];
    let mut changed = true;
    while changed {
        changed = false;
        for block in 0..blocks.len() {
            let new_in = if block == 0 || predecessors[block].is_empty() {
                entry_registers.clone()
            } else {
                let mut states = predecessors[block]
                    .iter()
                    .filter_map(|predecessor| outgoing[*predecessor].as_ref());
                let Some(first) = states.next() else {
                    continue;
                };
                states.fold(first.clone(), join_register_loan_states)
            };
            let mut new_out = new_in.clone();
            let mut generations = generation_entries[block].clone();
            for instruction in &blocks[block].instrs {
                transfer_register_loans(
                    &mut new_out,
                    &generations,
                    instruction,
                    loan_roots,
                    reg_types,
                );
                transfer_loan_generation(&mut generations, instruction, generation_dests);
            }
            if incoming[block].as_ref() != Some(&new_in)
                || outgoing[block].as_ref() != Some(&new_out)
            {
                incoming[block] = Some(new_in);
                outgoing[block] = Some(new_out);
                changed = true;
            }
        }
    }

    let entries: Vec<RegisterLoanState> = incoming
        .into_iter()
        .map(Option::unwrap_or_default)
        .collect();
    let uses = blocks
        .iter()
        .enumerate()
        .map(|(block, body)| {
            let mut registers = entries[block].clone();
            let mut generations = generation_entries[block].clone();
            body.instrs
                .iter()
                .map(|instruction| {
                    let uses = transfer_register_loans(
                        &mut registers,
                        &generations,
                        instruction,
                        loan_roots,
                        reg_types,
                    );
                    transfer_loan_generation(&mut generations, instruction, generation_dests);
                    uses
                })
                .collect()
        })
        .collect();
    (uses, entries)
}

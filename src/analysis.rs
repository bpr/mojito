//! Stage 6: ownership and persistent-loan analysis over MIR.
//!
//! Mojo's move semantics: transferring a value with `^` (`y = x^`, `take(x^)`)
//! leaves the source **uninitialized**, so using it again is an error. This pass
//! is a forward dataflow over each `MirFunction`'s basic blocks that tracks, per
//! variable, whether it is `Owned`, `Moved`, or — where control-flow paths
//! disagree — `MaybeMoved`. A use of a `Moved` variable is a **use-after-move**; a
//! use of a `MaybeMoved` one is a **conditional move** (transferred on some paths
//! but not others). Diagnostics carry the source [`Span`](crate::mir::Span) of the
//! offending use, recovered from the MIR `SpanTable`.
//!
//! This is a distinct compiler stage after MIR lowering. The production
//! [`Compiler`](crate::compiler::Compiler) always runs it before drop elaboration
//! and VM execution. Backward liveness then builds on this move/init foundation
//! to insert ASAP drops and control-flow-edge cleanup.

use crate::ast::Stmt;
use crate::error::OwnershipError;
use crate::hir::VarId;
use crate::mir::{
    MirBlock, MirCaptureMode, MirFunction, MirInstr, MirInteriorOrigin, MirPlace, MirProgram,
    MirTerm, Proj, Reg, SpanTable, UseMode, lower_program,
};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Run the ownership analysis over a whole program. Returns the first ownership
/// violation found (in function, then block, then instruction order), or `Ok` if
/// every value is used consistently with having been moved.
pub fn check_ownership(program: &[Stmt]) -> Result<(), OwnershipError> {
    let prog =
        lower_program(program).map_err(|error| OwnershipError::InvalidInput(error.to_string()))?;
    check_ownership_program(&prog)
}

pub fn check_ownership_checked(
    program: &crate::checked::CheckedProgram,
) -> Result<(), OwnershipError> {
    let prog = crate::mir::lower_checked_program(program);
    check_ownership_program(&prog)
}

/// Run the ownership analysis over an already-lowered program — the
/// standalone-MIR core the pipeline composes with `mir::verify` so production
/// MIR is fully verified before execution.
pub fn check_ownership_program(prog: &MirProgram) -> Result<(), OwnershipError> {
    for (_name, f) in &prog.functions {
        analyze_moves(f)?;
        analyze_interior_origins(f)?;
        analyze_loans(f)?;
    }
    Ok(())
}

// --- Liveness + ASAP drop elaboration ---------------------------------------

/// Elaborate ASAP destruction across a whole program: after each variable's last
/// use, splice a `DropVar`. Applied by the VM before execution so a struct's
/// `__del__` fires at the value's last use (not at scope end).
pub fn elaborate_drops_program(prog: MirProgram) -> MirProgram {
    MirProgram {
        functions: prog
            .functions
            .into_iter()
            .map(|(name, f)| {
                // Module-scope (`__toplevel__`) variables live until program end, so
                // they are not ASAP-dropped — that also keeps their final values
                // intact for the CLI/`bindings()` global dump (a `DropVar` would
                // clear the slot).
                let elaborated = if name == "__toplevel__" {
                    f
                } else {
                    elaborate_drops(&f)
                };
                (name, elaborated)
            })
            .collect(),
        declarations: prog.declarations,
        invariant_errors: prog.invariant_errors,
    }
}

/// The variables a MIR instruction reads, each paired with a nearby result
/// register (for a diagnostic span). Covers direct reads (`UseVar`), place roots
/// (`Store`/`LoadPlace`/a `mut self` receiver — so a write *through* a moved value
/// is caught too), and the `for` iterator variable.
fn var_uses(i: &MirInstr) -> Vec<(VarId, Reg)> {
    match i {
        MirInstr::EstablishLoans { loans, marker, .. } => loans
            .iter()
            .flat_map(|loan| place_loan_uses(&loan.place, *marker))
            .collect(),
        MirInstr::MakeRef { dest, place } => place_loan_uses(place, *dest),
        MirInstr::MakeClosure { dest, captures, .. } => captures
            .iter()
            .flat_map(|capture| place_loan_uses(&capture.place, *dest))
            .collect(),
        MirInstr::UseVar { dest, var, .. } => vec![(*var, *dest)],
        MirInstr::KeepAlive { var } => vec![(*var, Reg(0))],
        MirInstr::MovePlace { dest, place } => place_loan_uses(place, *dest),
        MirInstr::Store { place, src } => place_loan_uses(place, *src),
        MirInstr::StoreRef { place, reference } => place_loan_uses(place, *reference),
        MirInstr::MultiSet {
            receiver_place,
            arg_places,
            value_place,
            call,
            value,
            ..
        } => {
            let mut uses = receiver_place
                .iter()
                .chain(arg_places.iter().flatten())
                .chain(value_place.iter())
                .flat_map(|place| place_loan_uses(place, *value))
                .collect::<Vec<_>>();
            uses.extend(
                call.capture_accesses
                    .iter()
                    .map(|access| (access.root, *value)),
            );
            uses
        }
        MirInstr::VariantSet { place, value, .. } => place_loan_uses(place, *value),
        MirInstr::VariantReplace { place, value, .. } => place_loan_uses(place, *value),
        MirInstr::LoadPlace { dest, place } => place_loan_uses(place, *dest),
        // Call arguments are evaluated into registers first, but mutable/ref
        // conventions still consume their retained places at the call
        // boundary for handle passing and write-back. Keep those slots alive
        // through the instruction. (Interior stale-use checking deliberately
        // does not count this as a second value read.)
        MirInstr::Call {
            dest,
            arg_places,
            kwarg_places,
            capture_accesses,
            ..
        } => {
            let mut uses = arg_places
                .iter()
                .flatten()
                .chain(kwarg_places.iter().flatten())
                .flat_map(|place| place_loan_uses(place, *dest))
                .collect::<Vec<_>>();
            uses.extend(capture_accesses.iter().map(|access| (access.root, *dest)));
            uses
        }
        MirInstr::CallIndirect {
            dest,
            callee_place,
            arg_places,
            kwarg_places,
            capture_accesses,
            ..
        } => {
            let mut uses = callee_place
                .iter()
                .chain(arg_places.iter().flatten())
                .chain(kwarg_places.iter().flatten())
                .flat_map(|place| place_loan_uses(place, *dest))
                .collect::<Vec<_>>();
            uses.extend(capture_accesses.iter().map(|access| (access.root, *dest)));
            uses
        }
        MirInstr::MethodCall {
            dest,
            recv_place,
            arg_places,
            kwarg_places,
            capture_accesses,
            ..
        } => {
            let mut uses = recv_place
                .iter()
                .chain(arg_places.iter().flatten())
                .chain(kwarg_places.iter().flatten())
                .flat_map(|place| place_loan_uses(place, *dest))
                .collect::<Vec<_>>();
            uses.extend(capture_accesses.iter().map(|access| (access.root, *dest)));
            uses
        }
        MirInstr::Index {
            dest,
            base_place,
            index_place,
            call,
            ..
        } => {
            let mut uses = base_place
                .iter()
                .chain(index_place.iter())
                .flat_map(|place| place_loan_uses(place, *dest))
                .collect::<Vec<_>>();
            if let Some(call) = call {
                uses.extend(
                    call.capture_accesses
                        .iter()
                        .map(|access| (access.root, *dest)),
                );
            }
            uses
        }
        MirInstr::Slice {
            dest,
            object_place: base_place,
            arg_places: index_place,
            call,
            ..
        }
        | MirInstr::MultiIndex {
            dest,
            object_place: base_place,
            arg_places: index_place,
            call,
            ..
        } => {
            let mut uses = base_place
                .iter()
                .chain(index_place.iter().flatten())
                .flat_map(|place| place_loan_uses(place, *dest))
                .collect::<Vec<_>>();
            if let Some(call) = call {
                uses.extend(
                    call.capture_accesses
                        .iter()
                        .map(|access| (access.root, *dest)),
                );
            }
            uses
        }
        MirInstr::HasNext { dest, iter, .. }
        | MirInstr::Next { dest, iter, .. }
        | MirInstr::TryNext { dest, iter, .. } => {
            vec![(*iter, *dest)]
        }
        // `GetIter` normalizes its source into the iterator; count the source as a
        // use so a borrowed named source (a reference bound before the loop) stays
        // live through the read and is not dropped before the iterator is derived.
        MirInstr::GetIter { source, .. } => vec![(*source, Reg(0))],
        // A `try` reads every variable its sub-regions read: the outer liveness must
        // treat it as one big use, so a value used only inside the `try` is not
        // dropped *before* it.
        MirInstr::Try {
            body,
            handler,
            orelse,
            finalbody,
            ..
        } => {
            let mut uses = Vec::new();
            let mut add = |bs: &[MirBlock]| {
                for b in bs {
                    for instr in &b.instrs {
                        uses.extend(var_uses(instr));
                    }
                }
            };
            add(body);
            if let Some((_, h)) = handler {
                add(h);
            }
            if let Some(e) = orelse {
                add(e);
            }
            if let Some(fb) = finalbody {
                add(fb);
            }
            uses
        }
        _ => Vec::new(),
    }
}

fn place_loan_uses(place: &MirPlace, reg: Reg) -> Vec<(VarId, Reg)> {
    let mut uses = vec![(place.root, reg)];
    if let Some(reference) = place.through {
        uses.push((reference, reg));
    }
    uses
}

/// The variables **defined** within a `try` region's blocks (a `DefVar` at any
/// nesting), excluding those moved out with `^` — the body-local values to destroy
/// when the body is left (the exceptional-edge / scope-exit cleanup).
fn region_cleanup_vars(blocks: &[MirBlock]) -> Vec<VarId> {
    let mut defined: Vec<VarId> = Vec::new();
    let mut moved: HashSet<VarId> = HashSet::new();
    for b in blocks {
        for instr in &b.instrs {
            if let Some(v) = var_def(instr)
                && !defined.contains(&v)
            {
                defined.push(v);
            }
            for v in vars_moved(instr) {
                moved.insert(v);
            }
        }
    }
    defined.retain(|v| !moved.contains(v));
    defined
}

/// The variable a MIR instruction writes (a `DefVar`), if any.
fn var_def(i: &MirInstr) -> Option<VarId> {
    match i {
        MirInstr::DefVar { var, .. } => Some(*var),
        _ => None,
    }
}

/// `EstablishLoans` starts the reference's analytical live range, but it does not
/// overwrite the runtime handle already stored by `DefVar`.
fn loan_liveness_def(i: &MirInstr) -> Option<VarId> {
    match i {
        MirInstr::EstablishLoans { reference, .. } => Some(*reference),
        _ => var_def(i),
    }
}

/// The variable transferred out by this instruction (a `^` move), if any — such a
/// variable is *not* dropped here (its value has moved to a new owner).
fn vars_moved(i: &MirInstr) -> Vec<VarId> {
    match i {
        MirInstr::UseVar {
            var,
            mode: UseMode::Move,
            ..
        } => vec![*var],
        // A declaration-time move capture transfers a whole root into the
        // closure owner. Projected captures leave a residual aggregate that
        // still needs its ordinary drop, so only whole-root moves suppress it.
        MirInstr::MakeClosure { captures, .. } => captures
            .iter()
            .filter(|capture| capture.mode == MirCaptureMode::Move && capture.place.proj.is_empty())
            .map(|capture| capture.place.root)
            .collect(),
        _ => Vec::new(),
    }
}

/// Insert `DropVar`s at each variable's last use. A backward liveness dataflow
/// finds where each variable dies (touched, then dead), and a forward rebuild
/// splices a drop right after — skipping variables moved out with `^` (their new
/// owner drops them) so nothing is double-dropped. At a shared death point,
/// variables drop in reverse declaration order (descending `VarId`).
///
/// Conservative by design: it drops at a variable's last use *within a block* and
/// leaks (rather than risk a double-free) on the branch edges where a value dies
/// without being used — full drop elaboration across branches is future work.
/// Whether the value in variable `v` is dropped by *this* function: locals always;
/// a consuming `var` parameter (the caller transferred it) yes; a borrowed parameter or
/// `self` never (the caller owns a borrow; `self` is written back / would recurse).
fn is_droppable_root(f: &MirFunction, v: VarId) -> bool {
    let vi = v as usize;
    // `self` is always caller-owned here, including generated/specialized
    // method CFGs whose stable binding slot is not in the leading parameter
    // range.  Test this before `n_params`: classifying such a receiver as a
    // local makes drop elaboration recursively destroy borrowed fields (and,
    // for pointer-backed collections, free the caller's allocation).
    if f.var_names.get(vi).is_some_and(|name| name == "self") {
        return false;
    }
    if vi < f.n_params {
        // A consuming `var` parameter is destroyed here; a `deinit` parameter is
        // *consumed* here (its teardown is rewritten to `ConsumeVar` below).
        f.owned_params.get(vi).copied().unwrap_or(false)
            || f.deinit_params.get(vi).copied().unwrap_or(false)
    } else {
        true
    }
}

fn elaborate_drops(f: &MirFunction) -> MirFunction {
    let nb = f.blocks.len();
    let loan_roots = drop_loan_generations(f);
    let generation_entries = loan_generation_block_entries(f);
    let register_loan_uses = register_loan_uses(f, &generation_entries, &loan_roots);
    let generation_exits: Vec<LoanGenerationState> = f
        .blocks
        .iter()
        .zip(&generation_entries)
        .map(|(block, entry)| {
            let mut state = entry.clone();
            for instruction in &block.instrs {
                transfer_loan_generation(&mut state, instruction);
            }
            state
        })
        .collect();

    // First compute ordinary variable liveness without folding loan owners into
    // the sets. Owner retention is generation-sensitive, so adding it here would
    // lose path correlation at CFG joins and make every historical owner of a
    // rebound aggregate appear live forever.
    let mut live_in: Vec<HashSet<VarId>> = vec![HashSet::new(); nb];
    let mut changed = true;
    while changed {
        changed = false;
        for b in (0..nb).rev() {
            let live_out = block_live_out(f, b, &live_in);
            let new_in = transfer_drop_liveness(&f.blocks[b].instrs, live_out);
            if new_in != live_in[b] {
                live_in[b] = new_in;
                changed = true;
            }
        }
    }

    let effective_live_in: Vec<HashSet<VarId>> = live_in
        .iter()
        .zip(&generation_entries)
        .map(|(live, generations)| effective_drop_liveness(live.clone(), generations, &loan_roots))
        .collect();

    // (1) Block-internal drops: replay each block, tracking the live set after each
    // instruction, and drop the variables that die at their last use in-block.
    let mut blocks = Vec::with_capacity(nb);
    for b in 0..nb {
        let live_out = block_live_out(f, b, &live_in);
        let instrs = &f.blocks[b].instrs;
        let mut live = live_out.clone();
        let mut ordinary_live_after = vec![HashSet::new(); instrs.len()];
        let mut ordinary_live_before = vec![HashSet::new(); instrs.len()];
        for i in (0..instrs.len()).rev() {
            ordinary_live_after[i] = live.clone();
            if let Some(d) = var_def(&instrs[i]) {
                live.remove(&d);
            }
            for (u, _) in var_uses(&instrs[i]) {
                live.insert(u);
            }
            live.extend(&register_loan_uses[b][i]);
            ordinary_live_before[i] = live.clone();
        }

        let mut generation_state = generation_entries[b].clone();
        let mut generation_before = Vec::with_capacity(instrs.len());
        let mut generation_after = Vec::with_capacity(instrs.len());
        for instr in instrs {
            generation_before.push(generation_state.clone());
            transfer_loan_generation(&mut generation_state, instr);
            generation_after.push(generation_state.clone());
        }

        let mut live_before = Vec::with_capacity(instrs.len());
        let mut live_after = Vec::with_capacity(instrs.len());
        for i in 0..instrs.len() {
            live_before.push(effective_drop_liveness(
                ordinary_live_before[i].clone(),
                &generation_before[i],
                &loan_roots,
            ));
            live_after.push(effective_drop_liveness(
                ordinary_live_after[i].clone(),
                &generation_after[i],
                &loan_roots,
            ));
        }

        let mut new_instrs = Vec::with_capacity(instrs.len());
        for (i, instr) in instrs.iter().enumerate() {
            // For a `try`, fill its regions' escape-edge cleanups: the *outer*
            // variables live entering the `try` that are dead at the escape target
            // (so they die on the hidden `break`/`continue` edge), minus the `try`'s
            // own body-locals and moved-out values.
            let mut cloned = instr.clone();
            if let MirInstr::Try { .. } = &cloned {
                let (mut rdef, mut rmov) = (HashSet::new(), HashSet::new());
                try_region_defs(instr, &mut rdef, &mut rmov);
                // `finally` runs *after* every escape, so a variable it uses must
                // survive the escape edge — exclude it (and the loop var, which the
                // `finally` typically reads) from the escape cleanup.
                let fin_used = match instr {
                    MirInstr::Try {
                        finalbody: Some(fb),
                        ..
                    } => region_uses(fb),
                    _ => HashSet::new(),
                };
                let base: HashSet<VarId> = live_before[i]
                    .iter()
                    .copied()
                    .filter(|v| {
                        is_droppable_root(f, *v)
                            && !rdef.contains(v)
                            && !rmov.contains(v)
                            && !fin_used.contains(v)
                    })
                    .collect();
                fill_escape_cleanups(&mut cloned, &base, &effective_live_in);
            }
            new_instrs.push(cloned);
            let moved = vars_moved(instr);
            let mut dying: Vec<VarId> = Vec::new();
            let touched = var_uses(instr)
                .into_iter()
                .map(|(v, _)| v)
                .chain(var_def(instr))
                .chain(register_loan_uses[b][i].iter().copied());
            let roots_before = active_drop_loan_roots(&generation_before[i], &loan_roots);
            let roots_after = active_drop_loan_roots(&generation_after[i], &loan_roots);
            let retired_roots = roots_before.difference(&roots_after).copied();
            let deaths = live_before[i]
                .difference(&live_after[i])
                .copied()
                .chain(touched)
                .chain(retired_roots);
            for v in deaths {
                if is_droppable_root(f, v)
                    && !moved.contains(&v)
                    && !live_after[i].contains(&v)
                    && !dying.contains(&v)
                {
                    dying.push(v);
                }
            }
            append_drops(&mut new_instrs, dying);
        }
        if instrs.is_empty() && successors(&f.blocks[b].term).is_empty() {
            // A CFG may funnel live reference-bearing aggregates into an empty
            // return block. There is no final instruction to host the normal
            // before/after generation death, so retire those owners immediately
            // before the terminator.
            let dying =
                effective_drop_liveness(HashSet::new(), &generation_entries[b], &loan_roots)
                    .into_iter()
                    .filter(|owner| is_droppable_root(f, *owner))
                    .collect();
            append_drops(&mut new_instrs, dying);
        }
        blocks.push(MirBlock {
            instrs: new_instrs,
            term: f.blocks[b].term.clone(),
        });
    }

    // (2) Edge drops: a variable live out of `p` but dead entering successor `s`
    // dies on the edge `p → s` (e.g. a value used on one `if` arm but not the
    // other). Drop it on that edge — at the end of `p` (if `p` has one successor),
    // the start of `s` (if `s` has one predecessor), or, for a critical edge, in a
    // fresh block spliced between them.
    let pred_count = predecessor_counts(f);
    for p in 0..nb {
        // Unique successors (a `Branch`'s arms are distinct in practice; dedup for
        // safety so an edge isn't processed — and dropped — twice).
        let mut succs: Vec<usize> = successors(&f.blocks[p].term);
        succs.sort_unstable();
        succs.dedup();
        let n_succ = succs.len();
        let edge_live: Vec<(usize, HashSet<VarId>)> = succs
            .iter()
            .map(|successor| {
                (
                    *successor,
                    effective_drop_liveness(
                        live_in[*successor].clone(),
                        &generation_exits[p],
                        &loan_roots,
                    ),
                )
            })
            .collect();
        let live_out_p: HashSet<VarId> = edge_live
            .iter()
            .flat_map(|(_, live)| live.iter().copied())
            .collect();
        for &s in &succs {
            // Variables live out of `p` but dead entering `s` die on this edge.
            let live_on_edge = edge_live
                .iter()
                .find_map(|(successor, live)| (*successor == s).then_some(live))
                .expect("each successor has an effective edge-live set");
            let dying: Vec<VarId> = live_out_p
                .iter()
                .copied()
                .filter(|&v| !live_on_edge.contains(&v) && is_droppable_root(f, v))
                .collect();
            if dying.is_empty() {
                continue;
            }
            if n_succ == 1 {
                append_drops(&mut blocks[p].instrs, dying); // drop before the jump
            } else if pred_count[s] == 1 {
                prepend_drops(&mut blocks[s].instrs, dying); // drop on entry to `s`
            } else {
                // Critical edge: splice a drop block `p → new → s`.
                let new_idx = blocks.len();
                let mut instrs = Vec::new();
                append_drops(&mut instrs, dying);
                blocks.push(MirBlock {
                    instrs,
                    term: MirTerm::Jump(s),
                });
                rewire_target(&mut blocks[p].term, s, new_idx);
            }
        }
    }

    // Fill each `try`'s exceptional-edge cleanup (the body-local values to destroy
    // when the body is left), recursing into nested regions.
    set_try_cleanups(&mut blocks);

    // A `deinit` parameter is *consumed*, not destroyed: the spliced teardown
    // for it must skip the value's whole-value `__del__` (its resources already
    // moved into the receiver) while still destroying any residual fields. Drop
    // elaboration emits an ordinary `DropVar`; rewrite those to `ConsumeVar` for
    // deinit parameters. Their `^`-transferred fields are `Value::Moved` and a
    // no-op to drop, so this cannot double-free.
    if f.deinit_params.iter().any(|&d| d) {
        let is_deinit_param = |var: VarId| {
            (var as usize) < f.n_params
                && f.deinit_params.get(var as usize).copied().unwrap_or(false)
        };
        for block in &mut blocks {
            for instr in &mut block.instrs {
                if let MirInstr::DropVar { var } = instr
                    && is_deinit_param(*var)
                {
                    *instr = MirInstr::ConsumeVar { var: *var };
                }
            }
        }
    }

    MirFunction {
        blocks,
        n_regs: f.n_regs,
        n_vars: f.n_vars,
        var_names: f.var_names.clone(),
        n_params: f.n_params,
        param_types: f.param_types.clone(),
        owned_params: f.owned_params.clone(),
        deinit_params: f.deinit_params.clone(),
        ref_params: f.ref_params.clone(),
        returns_reference: f.returns_reference,
        var_tys: f.var_tys.clone(),
        ret_ty: f.ret_ty.clone(),
        raises: f.raises,
        error_ty: f.error_ty.clone(),
        spans: SpanTable(f.spans.0.clone()),
        reg_types: f.reg_types.clone(),
    }
}

/// The owner roots carried by each `EstablishLoans` generation. A later
/// establishment for the same aggregate replaces the old marker; keeping the
/// marker as the key prevents historical owners from becoming one permanent
/// dependency of the variable slot.
struct DropLoanGeneration {
    roots: Vec<VarId>,
    /// Direct `UnsafePointer` lowering reads the pointee before its ordinary
    /// variable use ends, so its existing NLL timing is already precise. `ref`
    /// values and reference-bearing aggregates can leave a handle in an SSA
    /// register and therefore participate in transient register provenance.
    propagate_through_registers: bool,
}

fn drop_loan_generations(f: &MirFunction) -> BTreeMap<u32, DropLoanGeneration> {
    let mut roots_by_generation: BTreeMap<u32, DropLoanGeneration> = BTreeMap::new();
    for instr in f.blocks.iter().flat_map(|block| &block.instrs) {
        if let MirInstr::EstablishLoans {
            reference,
            loans,
            marker,
        } = instr
        {
            let direct = matches!(
                f.var_tys.get(reference),
                Some(crate::types::Ty::Pointer { .. })
            );
            let generation =
                roots_by_generation
                    .entry(marker.0)
                    .or_insert_with(|| DropLoanGeneration {
                        roots: Vec::new(),
                        propagate_through_registers: !direct,
                    });
            for loan in loans {
                if !generation.roots.contains(&loan.place.root) {
                    generation.roots.push(loan.place.root);
                }
            }
        }
    }
    roots_by_generation
}

fn effective_drop_liveness(
    mut live: HashSet<VarId>,
    state: &LoanGenerationState,
    roots_by_generation: &BTreeMap<u32, DropLoanGeneration>,
) -> HashSet<VarId> {
    loop {
        let mut changed = false;
        for (reference, generations) in &state.active {
            let reference_live = live.contains(reference);
            for generation in generations {
                let Some(generation) = roots_by_generation.get(generation) else {
                    continue;
                };
                if reference_live {
                    for root in &generation.roots {
                        changed |= live.insert(*root);
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    live
}

fn active_drop_loan_roots(
    state: &LoanGenerationState,
    roots_by_generation: &BTreeMap<u32, DropLoanGeneration>,
) -> BTreeSet<VarId> {
    state
        .active
        .values()
        .flatten()
        .filter_map(|generation| roots_by_generation.get(generation))
        .flat_map(|generation| generation.roots.iter().copied())
        .collect()
}

#[derive(Clone, Default, PartialEq, Eq)]
struct RegisterLoanState {
    owners: BTreeMap<u32, BTreeSet<VarId>>,
}

fn join_register_loan_states(
    mut left: RegisterLoanState,
    right: &RegisterLoanState,
) -> RegisterLoanState {
    for (register, owners) in &right.owners {
        left.owners.entry(*register).or_default().extend(owners);
    }
    left
}

fn active_register_loan_roots(
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
fn transfer_register_loans(
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
    if let MirInstr::MakeRef { place, .. } = instruction {
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
    if let Some(place) = reference_subscript_place {
        owners.insert(place.root);
        if let Some(reference) = place.through {
            owners.insert(reference);
        }
    }

    let mut results = Vec::new();
    crate::mir::verify::instruction_result_regs(instruction, &mut results);
    for result in results {
        if owners.is_empty() {
            registers.owners.remove(&result.0);
        } else {
            registers.owners.insert(result.0, owners.clone());
        }
    }
    owners.into_iter().collect()
}

fn register_loan_uses(
    f: &MirFunction,
    generation_entries: &[LoanGenerationState],
    loan_roots: &BTreeMap<u32, DropLoanGeneration>,
) -> Vec<Vec<Vec<VarId>>> {
    let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); f.blocks.len()];
    for (block, body) in f.blocks.iter().enumerate() {
        for successor in successors(&body.term) {
            if successor < f.blocks.len() {
                predecessors[successor].push(block);
            }
        }
    }

    let mut incoming: Vec<Option<RegisterLoanState>> = vec![None; f.blocks.len()];
    let mut outgoing: Vec<Option<RegisterLoanState>> = vec![None; f.blocks.len()];
    let mut changed = true;
    while changed {
        changed = false;
        for block in 0..f.blocks.len() {
            let new_in = if block == 0 || predecessors[block].is_empty() {
                RegisterLoanState::default()
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
            for instruction in &f.blocks[block].instrs {
                transfer_register_loans(
                    &mut new_out,
                    &generations,
                    instruction,
                    loan_roots,
                    &f.reg_types,
                );
                transfer_loan_generation(&mut generations, instruction);
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

    f.blocks
        .iter()
        .enumerate()
        .map(|(block, body)| {
            let mut registers = incoming[block].clone().unwrap_or_default();
            let mut generations = generation_entries[block].clone();
            body.instrs
                .iter()
                .map(|instruction| {
                    let uses = transfer_register_loans(
                        &mut registers,
                        &generations,
                        instruction,
                        loan_roots,
                        &f.reg_types,
                    );
                    transfer_loan_generation(&mut generations, instruction);
                    uses
                })
                .collect()
        })
        .collect()
}

fn transfer_drop_liveness(instrs: &[MirInstr], mut live: HashSet<VarId>) -> HashSet<VarId> {
    for instr in instrs.iter().rev() {
        if let Some(d) = var_def(instr) {
            live.remove(&d);
        }
        for (u, _) in var_uses(instr) {
            live.insert(u);
        }
    }
    live
}

/// The variables *used* anywhere in a region's blocks (recursively, through nested
/// `try`s — `var_uses` already descends into a `Try` instruction).
fn region_uses(blocks: &[MirBlock]) -> HashSet<VarId> {
    let mut s = HashSet::new();
    for b in blocks {
        for instr in &b.instrs {
            for (v, _) in var_uses(instr) {
                s.insert(v);
            }
        }
    }
    s
}

/// Collect the variables *defined* or `^`-moved anywhere in a `try`'s regions
/// (recursively, through nested `try`s). Used to exclude a `try`'s own body-locals
/// (handled by `Try.cleanup`) and moved-out values from the escape-edge cleanup.
fn try_region_defs(try_instr: &MirInstr, defs: &mut HashSet<VarId>, moved: &mut HashSet<VarId>) {
    if let MirInstr::Try {
        body,
        handler,
        orelse,
        finalbody,
        ..
    } = try_instr
    {
        let mut regions: Vec<&Vec<MirBlock>> = vec![body];
        if let Some((_, h)) = handler {
            regions.push(h);
        }
        if let Some(e) = orelse {
            regions.push(e);
        }
        if let Some(fb) = finalbody {
            regions.push(fb);
        }
        for blocks in regions {
            for b in blocks {
                for instr in &b.instrs {
                    if let Some(v) = var_def(instr) {
                        defs.insert(v);
                    }
                    for v in vars_moved(instr) {
                        moved.insert(v);
                    }
                    try_region_defs(instr, defs, moved);
                }
            }
        }
    }
}

/// Fill each `EscapeJump.cleanup` inside a `try` (recursively) with the outer
/// variables from `base` that are dead at the escape's target block — those that
/// die on the hidden `break`/`continue` edge and must be destroyed there. `base`
/// already excludes the `try`'s body-locals (dropped by `Try.cleanup`), moved
/// values, and non-droppable roots.
fn fill_escape_cleanups(
    try_instr: &mut MirInstr,
    base: &HashSet<VarId>,
    live_in: &[HashSet<VarId>],
) {
    if let MirInstr::Try {
        body,
        handler,
        orelse,
        finalbody,
        ..
    } = try_instr
    {
        let mut regions: Vec<&mut Vec<MirBlock>> = vec![body];
        if let Some((_, h)) = handler {
            regions.push(h);
        }
        if let Some(e) = orelse {
            regions.push(e);
        }
        if let Some(fb) = finalbody {
            regions.push(fb);
        }
        for blocks in regions {
            for b in blocks.iter_mut() {
                for instr in b.instrs.iter_mut() {
                    fill_escape_cleanups(instr, base, live_in); // nested `try`s
                }
                if let MirTerm::EscapeJump { target, cleanup } = &mut b.term {
                    let dead_at_target = live_in.get(*target).cloned().unwrap_or_default();
                    let mut vars: Vec<VarId> = base
                        .iter()
                        .copied()
                        .filter(|v| !dead_at_target.contains(v))
                        .collect();
                    vars.sort_unstable_by(|a, b| b.cmp(a)); // reverse declaration order
                    *cleanup = vars;
                }
            }
        }
    }
}

/// Recursively fill every `MirInstr::Try`'s `cleanup` with the body's local
/// variables (dropped when the body is left, normally or via a raise).
fn set_try_cleanups(blocks: &mut [MirBlock]) {
    for b in blocks.iter_mut() {
        for instr in b.instrs.iter_mut() {
            if let MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                cleanup,
            } = instr
            {
                *cleanup = region_cleanup_vars(body);
                set_try_cleanups(body);
                if let Some((_, h)) = handler {
                    set_try_cleanups(h);
                }
                if let Some(e) = orelse {
                    set_try_cleanups(e);
                }
                if let Some(fb) = finalbody {
                    set_try_cleanups(fb);
                }
            }
        }
    }
}

/// The live-out set of block `b`: the union of its successors' live-in sets.
fn block_live_out(f: &MirFunction, b: usize, live_in: &[HashSet<VarId>]) -> HashSet<VarId> {
    let mut out = HashSet::new();
    for s in successors(&f.blocks[b].term) {
        out.extend(&live_in[s]);
    }
    out
}

/// Number of predecessors of each block (from terminator successors).
fn predecessor_counts(f: &MirFunction) -> Vec<usize> {
    let mut counts = vec![0usize; f.blocks.len()];
    for b in 0..f.blocks.len() {
        for s in successors(&f.blocks[b].term) {
            counts[s] += 1;
        }
    }
    counts
}

/// Append `DropVar`s for the given variables in reverse declaration order.
fn append_drops(instrs: &mut Vec<MirInstr>, mut vars: Vec<VarId>) {
    vars.sort_unstable_by(|a, b| b.cmp(a));
    for v in vars {
        instrs.push(MirInstr::DropVar { var: v });
    }
}

/// Prepend `DropVar`s (reverse declaration order) to the front of a block.
fn prepend_drops(instrs: &mut Vec<MirInstr>, mut vars: Vec<VarId>) {
    vars.sort_unstable_by(|a, b| b.cmp(a));
    for (i, v) in vars.into_iter().enumerate() {
        instrs.insert(i, MirInstr::DropVar { var: v });
    }
}

/// Redirect a terminator's `old` target to `new` (for critical-edge splitting).
fn rewire_target(term: &mut MirTerm, old: usize, new: usize) {
    match term {
        MirTerm::Jump(t) => {
            if *t == old {
                *t = new;
            }
        }
        MirTerm::Branch { then_b, else_b, .. } => {
            if *then_b == old {
                *then_b = new;
            }
            if *else_b == old {
                *else_b = new;
            }
        }
        // `EscapeJump` targets a block in the enclosing function; it never appears
        // as a *function-body* terminator (only inside a `try` region), and its
        // target isn't a critical-edge successor here, so leave it untouched.
        MirTerm::Return(_)
        | MirTerm::ReturnWithCleanup { .. }
        | MirTerm::FallOff
        | MirTerm::EscapeJump { .. } => {}
    }
}

/// Backward transfer over a block for liveness.
fn transfer_liveness(instrs: &[MirInstr], mut live: HashSet<VarId>) -> HashSet<VarId> {
    for instr in instrs.iter().rev() {
        if let Some(d) = loan_liveness_def(instr) {
            live.remove(&d);
        }
        for (u, _) in var_uses(instr) {
            live.insert(u);
        }
    }
    live
}

// --- Interior-origin generation analysis ----------------------------------

/// One `EstablishLoans` generation that contains at least one reference into
/// container-owned storage. The marker register is its stable identity; a
/// reference variable can name several possible markers after a CFG join.
#[derive(Clone)]
struct InteriorGeneration {
    origins: Vec<MirInteriorOrigin>,
}

#[derive(Clone, PartialEq, Eq)]
struct InteriorInvalidationState {
    at: crate::token::SourceSpan,
    origin: MirInteriorOrigin,
}

/// Forward may-state for interior origins. `active[reference]` is the set of
/// generations the reference variable may carry on paths reaching this point.
/// Invalidations are retained only while their generation remains active; this
/// preserves path correlation when one branch refreshes a reference after
/// invalidating it and another branch keeps the old, still-valid reference.
#[derive(Clone, PartialEq, Eq)]
struct InteriorState {
    /// `false` is the dataflow bottom used when a structured region has no
    /// normal (`FallOff`) exit. It prevents `return`/outer-loop escape paths
    /// from contaminating instructions that execute after a `try`.
    reachable: bool,
    active: BTreeMap<VarId, BTreeSet<u32>>,
    invalidated: BTreeMap<u32, InteriorInvalidationState>,
}

impl Default for InteriorState {
    fn default() -> Self {
        Self {
            reachable: true,
            active: BTreeMap::new(),
            invalidated: BTreeMap::new(),
        }
    }
}

impl InteriorState {
    fn unreachable() -> Self {
        Self {
            reachable: false,
            active: BTreeMap::new(),
            invalidated: BTreeMap::new(),
        }
    }
}

/// The three ways a structured region can complete. `normal` reaches the next
/// instruction, `raises` transfers to an enclosing handler, and `exits` carries
/// `return`/outer-loop `break`/`continue` paths. Keeping these channels separate
/// is what prevents non-fallthrough invalidations from leaking after a `try`.
#[derive(Clone)]
struct InteriorFlow {
    normal: InteriorState,
    raises: InteriorState,
    exits: InteriorState,
}

impl InteriorFlow {
    fn unreachable() -> Self {
        Self {
            normal: InteriorState::unreachable(),
            raises: InteriorState::unreachable(),
            exits: InteriorState::unreachable(),
        }
    }
}

/// Mojo interior origins are invalidation generations, not exclusive loans.
/// Ordinary reads and element writes may coexist with references into a
/// container, while a structural mutation makes every matching old generation
/// stale. This forward pass rejects a use when *any* path to it carries an
/// invalidated generation, including branch joins and loop backedges.
fn analyze_interior_origins(f: &MirFunction) -> Result<(), OwnershipError> {
    let generations = collect_interior_generations(&f.blocks);
    if generations.is_empty() {
        return Ok(());
    }
    check_interior_region_uses(InteriorState::default(), &f.blocks, &generations, f).map(|_| ())
}

fn collect_interior_generations(blocks: &[MirBlock]) -> BTreeMap<u32, InteriorGeneration> {
    fn collect(blocks: &[MirBlock], generations: &mut BTreeMap<u32, InteriorGeneration>) {
        for instr in blocks.iter().flat_map(|block| &block.instrs) {
            if let MirInstr::EstablishLoans { loans, marker, .. } = instr {
                let mut origins: Vec<_> = loans
                    .iter()
                    .filter_map(|loan| loan.interior.clone())
                    .collect();
                origins.sort();
                origins.dedup();
                if !origins.is_empty() {
                    generations.insert(marker.0, InteriorGeneration { origins });
                }
            }
            if let MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } = instr
            {
                collect(body, generations);
                if let Some((_, handler)) = handler {
                    collect(handler, generations);
                }
                if let Some(orelse) = orelse {
                    collect(orelse, generations);
                }
                if let Some(finalbody) = finalbody {
                    collect(finalbody, generations);
                }
            }
        }
    }

    let mut generations = BTreeMap::new();
    collect(blocks, &mut generations);
    generations
}

fn join_interior_states(mut left: InteriorState, right: &InteriorState) -> InteriorState {
    if !left.reachable {
        return right.clone();
    }
    if !right.reachable {
        return left;
    }
    for (reference, generations) in &right.active {
        left.active
            .entry(*reference)
            .or_default()
            .extend(generations);
    }
    for (generation, invalidation) in &right.invalidated {
        match left.invalidated.entry(*generation) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(invalidation.clone());
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                if invalidation_precedes(invalidation, entry.get()) {
                    entry.insert(invalidation.clone());
                }
            }
        }
    }
    left
}

fn invalidation_precedes(
    candidate: &InteriorInvalidationState,
    current: &InteriorInvalidationState,
) -> bool {
    candidate
        .at
        .source
        .cmp(&current.at.source)
        .then_with(|| candidate.at.span.cmp(&current.at.span))
        .then_with(|| candidate.origin.cmp(&current.origin))
        .is_lt()
}

fn transfer_interior_state(
    state: &mut InteriorState,
    instrs: &[MirInstr],
    generations: &BTreeMap<u32, InteriorGeneration>,
    f: &MirFunction,
) {
    for instr in instrs {
        if !state.reachable {
            break;
        }
        transfer_interior_instruction(state, instr, generations, f);
    }
}

fn transfer_interior_instruction(
    state: &mut InteriorState,
    instr: &MirInstr,
    generations: &BTreeMap<u32, InteriorGeneration>,
    f: &MirFunction,
) {
    match instr {
        MirInstr::DropVar { var } | MirInstr::ConsumeVar { var } => {
            remove_active_interior_reference(state, *var);
        }
        MirInstr::EstablishLoans {
            reference, marker, ..
        } => {
            remove_active_interior_reference(state, *reference);
            if generations.contains_key(&marker.0) {
                state.active.insert(*reference, BTreeSet::from([marker.0]));
            }
        }
        MirInstr::InvalidateInteriors {
            base,
            except,
            include_base_generation,
            marker,
        } => {
            let at = span_for_reg(f, *marker);
            for (reference, active) in &state.active {
                if Some(*reference) == *except {
                    continue;
                }
                for generation_id in active {
                    let Some(generation) = generations.get(generation_id) else {
                        continue;
                    };
                    let Some(origin) = generation.origins.iter().find(|origin| {
                        interior_origin_invalidated_by(origin, base, *include_base_generation)
                    }) else {
                        continue;
                    };
                    state.invalidated.entry(*generation_id).or_insert_with(|| {
                        InteriorInvalidationState {
                            at: at.clone(),
                            origin: origin.clone(),
                        }
                    });
                }
            }
        }
        MirInstr::Try { .. } => {
            *state = summarize_interior_try(state.clone(), instr, generations, f).normal;
        }
        MirInstr::Raise { .. } => state.reachable = false,
        _ => {}
    }
}

fn remove_active_interior_reference(state: &mut InteriorState, reference: VarId) {
    let Some(removed) = state.active.remove(&reference) else {
        return;
    };
    for generation in removed {
        if !state
            .active
            .values()
            .any(|active| active.contains(&generation))
        {
            state.invalidated.remove(&generation);
        }
    }
}

fn interior_region_states(
    entry: &InteriorState,
    blocks: &[MirBlock],
    generations: &BTreeMap<u32, InteriorGeneration>,
    f: &MirFunction,
) -> (Vec<Option<InteriorState>>, Vec<Option<InteriorState>>) {
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); blocks.len()];
    for (block, body) in blocks.iter().enumerate() {
        for successor in successors(&body.term) {
            if successor < blocks.len() {
                preds[successor].push(block);
            }
        }
    }
    let mut incoming: Vec<Option<InteriorState>> = vec![None; blocks.len()];
    let mut outgoing: Vec<Option<InteriorState>> = vec![None; blocks.len()];
    let mut changed = true;
    while changed {
        changed = false;
        for block in 0..blocks.len() {
            let new_in = if block == 0 {
                entry.clone()
            } else {
                let mut states = preds[block]
                    .iter()
                    .filter_map(|predecessor| outgoing[*predecessor].as_ref());
                let Some(first) = states.next() else {
                    continue;
                };
                states.fold(first.clone(), join_interior_states)
            };
            let mut new_out = new_in.clone();
            transfer_interior_state(&mut new_out, &blocks[block].instrs, generations, f);
            if incoming[block].as_ref() != Some(&new_in)
                || outgoing[block].as_ref() != Some(&new_out)
            {
                incoming[block] = Some(new_in);
                outgoing[block] = Some(new_out);
                changed = true;
            }
        }
    }
    (incoming, outgoing)
}

fn add_interior_state(target: &mut InteriorState, source: &InteriorState) {
    *target = join_interior_states(target.clone(), source);
}

fn joined_interior_flow_inputs(flow: &InteriorFlow) -> InteriorState {
    let joined = join_interior_states(flow.normal.clone(), &flow.raises);
    join_interior_states(joined, &flow.exits)
}

fn summarize_interior_region(
    entry: InteriorState,
    blocks: &[MirBlock],
    generations: &BTreeMap<u32, InteriorGeneration>,
    f: &MirFunction,
) -> InteriorFlow {
    if blocks.is_empty() {
        return InteriorFlow {
            normal: entry,
            raises: InteriorState::unreachable(),
            exits: InteriorState::unreachable(),
        };
    }
    let (incoming, _) = interior_region_states(&entry, blocks, generations, f);
    let mut flow = InteriorFlow::unreachable();
    for (block, body) in blocks.iter().enumerate() {
        let Some(mut state) = incoming[block].clone() else {
            continue;
        };
        if !state.reachable {
            continue;
        }
        for instr in &body.instrs {
            if matches!(instr, MirInstr::Try { .. }) {
                let nested = summarize_interior_try(state, instr, generations, f);
                add_interior_state(&mut flow.raises, &nested.raises);
                add_interior_state(&mut flow.exits, &nested.exits);
                state = nested.normal;
            } else {
                if interior_instruction_directly_raises(instr) {
                    add_interior_state(&mut flow.raises, &state);
                }
                transfer_interior_instruction(&mut state, instr, generations, f);
            }
            if !state.reachable {
                break;
            }
        }
        if !state.reachable {
            continue;
        }
        match body.term {
            MirTerm::FallOff => add_interior_state(&mut flow.normal, &state),
            MirTerm::Return(_) | MirTerm::ReturnWithCleanup { .. } | MirTerm::EscapeJump { .. } => {
                add_interior_state(&mut flow.exits, &state);
            }
            MirTerm::Jump(_) | MirTerm::Branch { .. } => {}
        }
    }
    flow
}

/// Replay one CFG from its fixed-point states, recursively checking every
/// nested `try` region. Effects occur after uses: borrowing through a stale
/// reference is rejected before an establishment can replace its generation.
fn check_interior_region_uses(
    entry: InteriorState,
    blocks: &[MirBlock],
    generations: &BTreeMap<u32, InteriorGeneration>,
    f: &MirFunction,
) -> Result<InteriorFlow, OwnershipError> {
    let summary = summarize_interior_region(entry.clone(), blocks, generations, f);
    if blocks.is_empty() {
        return Ok(summary);
    }
    let (incoming, _) = interior_region_states(&entry, blocks, generations, f);
    for (block, body) in blocks.iter().enumerate() {
        let Some(mut state) = incoming[block].clone() else {
            continue;
        };
        if !state.reachable {
            continue;
        }
        for instr in &body.instrs {
            if matches!(instr, MirInstr::Try { .. }) {
                state = check_interior_try_uses(state, instr, generations, f)?.normal;
            } else {
                check_interior_instruction_uses(&state, instr, f)?;
                transfer_interior_instruction(&mut state, instr, generations, f);
            }
            if !state.reachable {
                break;
            }
        }
    }
    Ok(summary)
}

fn check_interior_instruction_uses(
    state: &InteriorState,
    instr: &MirInstr,
    f: &MirFunction,
) -> Result<(), OwnershipError> {
    for (reference, marker) in interior_reference_uses(instr) {
        let Some(active) = state.active.get(&reference) else {
            continue;
        };
        if let Some(invalidation) = active
            .iter()
            .find_map(|generation| state.invalidated.get(generation))
        {
            return Err(OwnershipError::InvalidatedInteriorReference {
                reference: var_name(f, reference),
                origin: interior_origin_display(f, &invalidation.origin),
                span: span_for_reg(f, marker),
                invalidated_at: Box::new(invalidation.at.clone()),
            });
        }
    }
    Ok(())
}

fn interior_instruction_directly_raises(instr: &MirInstr) -> bool {
    matches!(
        instr,
        MirInstr::Raise { .. }
            | MirInstr::Call {
                raises: Some(_),
                ..
            }
            | MirInstr::CallIndirect {
                raises: Some(_),
                ..
            }
            | MirInstr::MethodCall {
                raises: Some(_),
                ..
            }
            | MirInstr::Index {
                call: Some(crate::mir::MirSubscriptCall {
                    raises: Some(_),
                    ..
                }),
                ..
            }
            | MirInstr::Slice {
                call: Some(crate::mir::MirSubscriptCall {
                    raises: Some(_),
                    ..
                }),
                ..
            }
            | MirInstr::MultiIndex {
                call: Some(crate::mir::MirSubscriptCall {
                    raises: Some(_),
                    ..
                }),
                ..
            }
            | MirInstr::MultiSet {
                call: crate::mir::MirSubscriptCall {
                    raises: Some(_),
                    ..
                },
                ..
            }
    )
}

fn summarize_interior_try(
    entry: InteriorState,
    instr: &MirInstr,
    generations: &BTreeMap<u32, InteriorGeneration>,
    f: &MirFunction,
) -> InteriorFlow {
    let MirInstr::Try {
        body,
        handler,
        orelse,
        finalbody,
        ..
    } = instr
    else {
        return InteriorFlow {
            normal: entry,
            raises: InteriorState::unreachable(),
            exits: InteriorState::unreachable(),
        };
    };

    let body_flow = summarize_interior_region(entry, body, generations, f);
    let mut normal = InteriorState::unreachable();
    let mut raises = InteriorState::unreachable();
    let mut exits = body_flow.exits.clone();

    if let Some(orelse) = orelse {
        let else_flow = summarize_interior_region(body_flow.normal, orelse, generations, f);
        add_interior_state(&mut normal, &else_flow.normal);
        add_interior_state(&mut raises, &else_flow.raises);
        add_interior_state(&mut exits, &else_flow.exits);
    } else {
        add_interior_state(&mut normal, &body_flow.normal);
    }

    if let Some((_, handler)) = handler {
        let handler_flow = summarize_interior_region(body_flow.raises, handler, generations, f);
        add_interior_state(&mut normal, &handler_flow.normal);
        add_interior_state(&mut raises, &handler_flow.raises);
        add_interior_state(&mut exits, &handler_flow.exits);
    } else {
        add_interior_state(&mut raises, &body_flow.raises);
    }

    apply_interior_finally(
        InteriorFlow {
            normal,
            raises,
            exits,
        },
        finalbody.as_deref(),
        generations,
        f,
    )
}

fn check_interior_try_uses(
    entry: InteriorState,
    instr: &MirInstr,
    generations: &BTreeMap<u32, InteriorGeneration>,
    f: &MirFunction,
) -> Result<InteriorFlow, OwnershipError> {
    let MirInstr::Try {
        body,
        handler,
        orelse,
        finalbody,
        ..
    } = instr
    else {
        return Ok(InteriorFlow {
            normal: entry,
            raises: InteriorState::unreachable(),
            exits: InteriorState::unreachable(),
        });
    };

    let body_flow = check_interior_region_uses(entry, body, generations, f)?;
    let mut normal = InteriorState::unreachable();
    let mut raises = InteriorState::unreachable();
    let mut exits = body_flow.exits.clone();

    if let Some(orelse) = orelse {
        let else_flow = check_interior_region_uses(body_flow.normal, orelse, generations, f)?;
        add_interior_state(&mut normal, &else_flow.normal);
        add_interior_state(&mut raises, &else_flow.raises);
        add_interior_state(&mut exits, &else_flow.exits);
    } else {
        add_interior_state(&mut normal, &body_flow.normal);
    }

    if let Some((_, handler)) = handler {
        let handler_flow = check_interior_region_uses(body_flow.raises, handler, generations, f)?;
        add_interior_state(&mut normal, &handler_flow.normal);
        add_interior_state(&mut raises, &handler_flow.raises);
        add_interior_state(&mut exits, &handler_flow.exits);
    } else {
        add_interior_state(&mut raises, &body_flow.raises);
    }

    let flow = InteriorFlow {
        normal,
        raises,
        exits,
    };
    if let Some(finalbody) = finalbody {
        // `finally` executes for normal, exceptional, and return/escape paths;
        // checking it from their joined input catches a stale use on any one of
        // those paths. Only its normal-channel input can reach after the `try`.
        let all_inputs = joined_interior_flow_inputs(&flow);
        let _ = check_interior_region_uses(all_inputs, finalbody, generations, f)?;
    }
    Ok(apply_interior_finally(
        flow,
        finalbody.as_deref(),
        generations,
        f,
    ))
}

fn apply_interior_finally(
    flow: InteriorFlow,
    finalbody: Option<&[MirBlock]>,
    generations: &BTreeMap<u32, InteriorGeneration>,
    f: &MirFunction,
) -> InteriorFlow {
    let Some(finalbody) = finalbody else {
        return flow;
    };

    let normal_final = summarize_interior_region(flow.normal, finalbody, generations, f);
    let raising_final = summarize_interior_region(flow.raises, finalbody, generations, f);
    let exiting_final = summarize_interior_region(flow.exits, finalbody, generations, f);

    let mut raises = raising_final.normal;
    add_interior_state(&mut raises, &normal_final.raises);
    add_interior_state(&mut raises, &raising_final.raises);
    add_interior_state(&mut raises, &exiting_final.raises);

    let mut exits = exiting_final.normal;
    add_interior_state(&mut exits, &normal_final.exits);
    add_interior_state(&mut exits, &raising_final.exits);
    add_interior_state(&mut exits, &exiting_final.exits);

    InteriorFlow {
        normal: normal_final.normal,
        raises,
        exits,
    }
}

fn interior_origin_invalidated_by(
    origin: &MirInteriorOrigin,
    base: &MirInteriorOrigin,
    include_base_generation: bool,
) -> bool {
    if origin.root != base.root || base.path.len() > origin.path.len() {
        return false;
    }
    let prefix_matches = base.path.iter().zip(&origin.path).all(|(left, right)| {
        left == right
            || matches!(left, crate::origin::OriginSeg::AnyIndex)
            || matches!(right, crate::origin::OriginSeg::AnyIndex)
    });
    prefix_matches
        && (include_base_generation
            || origin.path[base.path.len()..]
                .iter()
                .any(|segment| matches!(segment, crate::origin::OriginSeg::Interior(_))))
}

/// Every reference variable semantically consumed by an instruction, paired
/// with a register carrying the source span for the diagnostic. Place roots are
/// included as well as `through`: roots can themselves be reference-bearing
/// aggregate slots, while an ordinary reference place normally names its owner
/// root and records the actual reference in `through`.
fn interior_reference_uses(instr: &MirInstr) -> Vec<(VarId, Reg)> {
    fn add_place(uses: &mut Vec<(VarId, Reg)>, place: &MirPlace, marker: Reg) {
        uses.push((place.root, marker));
        if let Some(reference) = place.through {
            uses.push((reference, marker));
        }
    }

    fn add_subscript_call_places(
        uses: &mut Vec<(VarId, Reg)>,
        call: &crate::mir::MirSubscriptCall,
        receiver_place: Option<&MirPlace>,
        positional_places: &[Option<MirPlace>],
        keyword_places: &[Option<MirPlace>],
        marker: Reg,
    ) {
        if call.receiver_requires_place
            && let Some(place) = receiver_place
        {
            add_place(uses, place, marker);
        }
        for argument in &call.arguments {
            if !argument.requires_place {
                continue;
            }
            let place = match argument.source {
                crate::checked::CheckedCallArgumentSource::Positional(index) => {
                    positional_places.get(index).and_then(Option::as_ref)
                }
                crate::checked::CheckedCallArgumentSource::Keyword(index) => {
                    keyword_places.get(index).and_then(Option::as_ref)
                }
                crate::checked::CheckedCallArgumentSource::Default => None,
            };
            if let Some(place) = place {
                add_place(uses, place, marker);
            }
        }
    }

    let mut uses = Vec::new();
    match instr {
        MirInstr::EstablishLoans { loans, marker, .. } => {
            for loan in loans {
                add_place(&mut uses, &loan.place, *marker);
            }
        }
        MirInstr::MakeRef { dest, place }
        | MirInstr::MovePlace { dest, place }
        | MirInstr::LoadPlace { dest, place } => add_place(&mut uses, place, *dest),
        MirInstr::MakeClosure { dest, captures, .. } => {
            for capture in captures {
                add_place(&mut uses, &capture.place, *dest);
            }
        }
        MirInstr::UseVar { dest, var, .. } => uses.push((*var, *dest)),
        MirInstr::Store { place, src } => add_place(&mut uses, place, *src),
        MirInstr::StoreRef { place, reference } => {
            add_place(&mut uses, place, *reference);
        }
        MirInstr::MultiSet {
            receiver_place,
            arg_places,
            value_place,
            value,
            value_keyword,
            call,
            ..
        } => {
            let mut positional = arg_places.clone();
            let keyword = if *value_keyword {
                vec![value_place.clone()]
            } else {
                positional.push(value_place.clone());
                Vec::new()
            };
            add_subscript_call_places(
                &mut uses,
                call,
                receiver_place.as_ref(),
                &positional,
                &keyword,
                *value,
            );
        }
        MirInstr::Index {
            dest,
            base_place,
            index_place,
            call: Some(call),
            ..
        } => add_subscript_call_places(
            &mut uses,
            call,
            base_place.as_ref(),
            std::slice::from_ref(index_place),
            &[],
            *dest,
        ),
        MirInstr::Slice {
            dest,
            object_place,
            arg_places,
            call: Some(call),
            ..
        }
        | MirInstr::MultiIndex {
            dest,
            object_place,
            arg_places,
            call: Some(call),
            ..
        } => add_subscript_call_places(
            &mut uses,
            call,
            object_place.as_ref(),
            arg_places,
            &[],
            *dest,
        ),
        MirInstr::VariantSet { place, value, .. }
        | MirInstr::VariantReplace { place, value, .. } => {
            add_place(&mut uses, place, *value);
        }
        MirInstr::ConsumePlace { place, marker } => add_place(&mut uses, place, *marker),
        MirInstr::HasNext { dest, iter, .. }
        | MirInstr::Next { dest, iter, .. }
        | MirInstr::TryNext { dest, iter, .. } => {
            uses.push((*iter, *dest));
        }
        // Call argument and receiver registers were evaluated before the
        // call-boundary invalidation. `arg_places`/`recv_place` are write-back
        // metadata, not a second read of a copied argument at call time.
        MirInstr::Call { .. } | MirInstr::CallIndirect { .. } | MirInstr::MethodCall { .. } => {}
        // Invalidating, dropping, or keeping an analytical handle alive is not
        // a read through that handle. `Try` regions are replayed separately.
        MirInstr::InvalidateInteriors { .. }
        | MirInstr::DefVar { .. }
        | MirInstr::DropVar { .. }
        | MirInstr::ConsumeVar { .. }
        | MirInstr::KeepAlive { .. }
        | MirInstr::Try { .. }
        | MirInstr::Const { .. }
        | MirInstr::MaterializeLiteral { .. }
        | MirInstr::ReadRef { .. }
        | MirInstr::CopyValue { .. }
        | MirInstr::WriteRef { .. }
        | MirInstr::UnOp { .. }
        | MirInstr::BinOp { .. }
        | MirInstr::GetField { .. }
        | MirInstr::Index { call: None, .. }
        | MirInstr::Slice { call: None, .. }
        | MirInstr::MultiIndex { call: None, .. }
        | MirInstr::MakeTuple { .. }
        | MirInstr::MakeVariant { .. }
        | MirInstr::VariantIs { .. }
        | MirInstr::VariantGet { .. }
        | MirInstr::VariantTake { .. }
        | MirInstr::PointerStorageTake { .. }
        | MirInstr::PointerStorageDestroy { .. }
        | MirInstr::MakeSimd { .. }
        | MirInstr::Raise { .. }
        | MirInstr::Drop { .. }
        | MirInstr::Unsupported(_)
        | MirInstr::GetIter { .. } => {}
    }
    uses.sort_unstable_by_key(|(reference, _)| *reference);
    uses.dedup_by_key(|(reference, _)| *reference);
    uses
}

fn span_for_reg(f: &MirFunction, reg: Reg) -> crate::token::SourceSpan {
    f.spans
        .0
        .get(&reg.0)
        .map(|(span, _)| span.clone())
        .unwrap_or_else(|| crate::token::SourceSpan::new(None, crate::token::DUMMY_SPAN))
}

fn var_name(f: &MirFunction, var: VarId) -> String {
    f.var_names
        .get(var as usize)
        .cloned()
        .unwrap_or_else(|| format!("${var}"))
}

fn interior_origin_display(f: &MirFunction, origin: &MirInteriorOrigin) -> String {
    let mut display = var_name(f, origin.root);
    for segment in &origin.path {
        match segment {
            crate::origin::OriginSeg::Field(field) => {
                display.push('.');
                display.push_str(field);
            }
            crate::origin::OriginSeg::AnyIndex => display.push_str("[…]"),
            crate::origin::OriginSeg::Interior(tag) => {
                display.push_str("[\"");
                display.push_str(tag);
                display.push_str("\"]");
            }
        }
    }
    display
}

#[derive(Clone)]
struct Loan {
    place: MirPlace,
    mutable: bool,
    interior: Option<MirInteriorOrigin>,
}

/// The loan generation(s) a reference-bearing slot may currently contain.
/// Each `EstablishLoans` marker is a reaching definition: rebinding replaces
/// the previous generation on that path, while CFG joins retain every possible
/// incoming generation.
#[derive(Clone, Default, PartialEq, Eq)]
struct LoanGenerationState {
    active: BTreeMap<VarId, BTreeSet<u32>>,
}

fn join_loan_generation_states(
    mut left: LoanGenerationState,
    right: &LoanGenerationState,
) -> LoanGenerationState {
    for (reference, generations) in &right.active {
        left.active
            .entry(*reference)
            .or_default()
            .extend(generations);
    }
    left
}

fn transfer_loan_generation(state: &mut LoanGenerationState, instruction: &MirInstr) {
    match instruction {
        MirInstr::EstablishLoans {
            reference, marker, ..
        } => {
            state.active.insert(*reference, BTreeSet::from([marker.0]));
        }
        // A definition with no following `EstablishLoans` replaces a
        // reference-bearing value with one carrying no owner dependency.
        MirInstr::DefVar { var, .. } | MirInstr::DropVar { var } | MirInstr::ConsumeVar { var } => {
            state.active.remove(var);
        }
        _ => {}
    }
}

fn loan_generation_block_entries(f: &MirFunction) -> Vec<LoanGenerationState> {
    let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); f.blocks.len()];
    for (block, body) in f.blocks.iter().enumerate() {
        for successor in successors(&body.term) {
            if successor < f.blocks.len() {
                predecessors[successor].push(block);
            }
        }
    }
    let mut incoming: Vec<Option<LoanGenerationState>> = vec![None; f.blocks.len()];
    let mut outgoing: Vec<Option<LoanGenerationState>> = vec![None; f.blocks.len()];
    let mut changed = true;
    while changed {
        changed = false;
        for block in 0..f.blocks.len() {
            let new_in = if block == 0 || predecessors[block].is_empty() {
                LoanGenerationState::default()
            } else {
                let mut states = predecessors[block]
                    .iter()
                    .filter_map(|predecessor| outgoing[*predecessor].as_ref());
                let Some(first) = states.next() else {
                    continue;
                };
                states.fold(first.clone(), join_loan_generation_states)
            };
            let mut new_out = new_in.clone();
            for instruction in &f.blocks[block].instrs {
                transfer_loan_generation(&mut new_out, instruction);
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
    incoming
        .into_iter()
        .map(Option::unwrap_or_default)
        .collect()
}

fn reaching_loans<'a>(
    state: &LoanGenerationState,
    generations: &'a BTreeMap<u32, Vec<Loan>>,
    reference: VarId,
) -> Vec<&'a Loan> {
    state
        .active
        .get(&reference)
        .into_iter()
        .flatten()
        .filter_map(|generation| generations.get(generation))
        .flatten()
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoanAccess {
    Read,
    Write,
}

/// Persistent local-loan checking. Reference variables participate in ordinary
/// backward liveness through `MirPlace::through`, so a loan is active precisely
/// from `EstablishLoans` through the reference's last use, including CFG
/// joins/loops. Interior-storage loans have generation semantics instead of
/// exclusive-owner semantics; the forward interior-origin pass checks those.
fn analyze_loans(f: &MirFunction) -> Result<(), OwnershipError> {
    let mut generations: BTreeMap<u32, Vec<Loan>> = BTreeMap::new();
    for instr in f.blocks.iter().flat_map(|block| &block.instrs) {
        if let MirInstr::EstablishLoans {
            loans: established,
            marker,
            ..
        } = instr
        {
            generations.insert(
                marker.0,
                established
                    .iter()
                    .map(|loan| Loan {
                        place: loan.place.clone(),
                        mutable: loan.mutable,
                        interior: loan.interior.clone(),
                    })
                    .collect(),
            );
        }
    }
    if generations.is_empty() {
        return Ok(());
    }

    let nb = f.blocks.len();
    let generation_entries = loan_generation_block_entries(f);
    let mut live_in = vec![HashSet::new(); nb];
    let mut changed = true;
    while changed {
        changed = false;
        for block in (0..nb).rev() {
            let live_out = block_live_out(f, block, &live_in);
            let incoming = transfer_liveness(&f.blocks[block].instrs, live_out);
            if incoming != live_in[block] {
                live_in[block] = incoming;
                changed = true;
            }
        }
    }

    for (block, generation_entry) in generation_entries.iter().enumerate().take(nb) {
        let instrs = &f.blocks[block].instrs;
        let mut generation_state = generation_entry.clone();
        let mut live = block_live_out(f, block, &live_in);
        let mut live_before = vec![HashSet::new(); instrs.len()];
        for index in (0..instrs.len()).rev() {
            if let Some(def) = loan_liveness_def(&instrs[index]) {
                live.remove(&def);
            }
            for (used, _) in var_uses(&instrs[index]) {
                live.insert(used);
            }
            live_before[index] = live.clone();
        }

        for (index, instr) in instrs.iter().enumerate() {
            let active = &live_before[index];
            if let MirInstr::EstablishLoans {
                reference,
                loans: established,
                marker,
            } = instr
            {
                for loan in established.iter().filter(|loan| loan.interior.is_none()) {
                    for other in active.iter().filter(|id| **id != *reference) {
                        if loan.place.through == Some(*other) {
                            // A reborrow derives its permission from this live
                            // reference instead of competing with it. Mutable
                            // capability still cannot be recovered from a
                            // shared source reference.
                            if loan.mutable
                                && reaching_loans(&generation_state, &generations, *other)
                                    .iter()
                                    .any(|source| !source.mutable)
                            {
                                let span = f
                                    .spans
                                    .0
                                    .get(&marker.0)
                                    .map(|(span, _)| span.clone())
                                    .unwrap_or_else(|| {
                                        crate::token::SourceSpan::new(
                                            None,
                                            crate::token::DUMMY_SPAN,
                                        )
                                    });
                                return Err(loan_error(f, &loan.place, *other, span));
                            }
                            continue;
                        }
                        if reaching_loans(&generation_state, &generations, *other)
                            .iter()
                            .any(|existing| {
                                existing.interior.is_none()
                                    && (loan.mutable || existing.mutable)
                                    && mir_places_overlap(&loan.place, &existing.place)
                            })
                        {
                            let span = f
                                .spans
                                .0
                                .get(&marker.0)
                                .map(|(span, _)| span.clone())
                                .unwrap_or_else(|| {
                                    crate::token::SourceSpan::new(None, crate::token::DUMMY_SPAN)
                                });
                            return Err(loan_error(f, &loan.place, *other, span));
                        }
                    }
                }
                transfer_loan_generation(&mut generation_state, instr);
                continue;
            }
            for (place, access, span) in loan_accesses(f, instr) {
                for reference in active {
                    let reference_loans =
                        reaching_loans(&generation_state, &generations, *reference);
                    if reference_loans.is_empty() {
                        continue;
                    }
                    if place.through == Some(*reference) {
                        if access == LoanAccess::Write
                            && reference_loans.iter().any(|loan| !loan.mutable)
                        {
                            return Err(loan_error(f, &place, *reference, span));
                        }
                        continue;
                    }
                    if reference_loans.iter().any(|loan| {
                        loan.interior.is_none()
                            && mir_places_overlap(&place, &loan.place)
                            && (access == LoanAccess::Write || loan.mutable)
                    }) {
                        return Err(loan_error(f, &place, *reference, span));
                    }
                }
            }
            transfer_loan_generation(&mut generation_state, instr);
        }
    }
    Ok(())
}

fn mir_places_overlap(left: &MirPlace, right: &MirPlace) -> bool {
    left.root == right.root
        && left.proj.iter().zip(&right.proj).all(|(a, b)| {
            matches!((a, b), (Proj::Field(x), Proj::Field(y)) if x == y)
                || matches!((a, b), (Proj::Index(_), Proj::Index(_)))
                || matches!(
                    (a, b),
                    (Proj::Index(_), Proj::ConstIndex(_)) | (Proj::ConstIndex(_), Proj::Index(_))
                )
                || matches!((a, b), (Proj::ConstIndex(x), Proj::ConstIndex(y)) if x == y)
                || matches!((a, b), (Proj::Variant(x), Proj::Variant(y)) if x == y)
        })
}

fn loan_accesses(
    f: &MirFunction,
    instr: &MirInstr,
) -> Vec<(MirPlace, LoanAccess, crate::token::SourceSpan)> {
    let fallback = crate::token::SourceSpan::new(None, crate::token::DUMMY_SPAN);
    let span_for = |reg: Reg| {
        f.spans
            .0
            .get(&reg.0)
            .map(|(span, _)| span.clone())
            .unwrap_or_else(|| fallback.clone())
    };
    let captured = |access: &crate::mir::MirCaptureAccess, marker: Reg| {
        let mut place = MirPlace::root(access.root, None);
        // A concrete field prefix improves precision. Abstract indices and
        // interior-storage markers collapse to their owner prefix, which is the
        // conservative overlap relation required for call-side effects.
        for segment in &access.path {
            match segment {
                crate::origin::OriginSeg::Field(field) => {
                    place.proj.push(Proj::Field(field.clone()));
                }
                crate::origin::OriginSeg::AnyIndex | crate::origin::OriginSeg::Interior(_) => break,
            }
        }
        (
            place,
            if access.access == crate::origin::CaptureAccess::Write {
                LoanAccess::Write
            } else {
                LoanAccess::Read
            },
            span_for(marker),
        )
    };
    let access_for_convention = |convention: Option<crate::ast::ArgConvention>| {
        if matches!(
            convention,
            Some(
                crate::ast::ArgConvention::Mut
                    | crate::ast::ArgConvention::Ref
                    | crate::ast::ArgConvention::Var
                    | crate::ast::ArgConvention::Out
                    | crate::ast::ArgConvention::Deinit
            )
        ) {
            LoanAccess::Write
        } else {
            LoanAccess::Read
        }
    };
    let subscript_accesses = |call: &crate::mir::MirSubscriptCall,
                              receiver_place: Option<&MirPlace>,
                              positional_places: &[Option<MirPlace>],
                              keyword_places: &[Option<MirPlace>],
                              marker: Reg| {
        let mut accesses = Vec::new();
        if let Some(place) = receiver_place {
            accesses.push((
                place.clone(),
                access_for_convention(call.receiver_convention),
                span_for(marker),
            ));
        }
        for argument in &call.arguments {
            let place = match argument.source {
                crate::checked::CheckedCallArgumentSource::Positional(index) => {
                    positional_places.get(index).and_then(Option::as_ref)
                }
                crate::checked::CheckedCallArgumentSource::Keyword(index) => {
                    keyword_places.get(index).and_then(Option::as_ref)
                }
                crate::checked::CheckedCallArgumentSource::Default => None,
            };
            if let Some(place) = place {
                accesses.push((
                    place.clone(),
                    access_for_convention(argument.convention),
                    span_for(marker),
                ));
            }
        }
        accesses.extend(
            call.capture_accesses
                .iter()
                .map(|access| captured(access, marker)),
        );
        accesses
    };
    match instr {
        MirInstr::UseVar { var, dest, mode } => vec![(
            MirPlace::root(*var, None),
            if matches!(mode, UseMode::Move) {
                LoanAccess::Write
            } else {
                LoanAccess::Read
            },
            span_for(*dest),
        )],
        MirInstr::DefVar { var, src, .. } => vec![(
            MirPlace::root(*var, None),
            LoanAccess::Write,
            span_for(*src),
        )],
        MirInstr::LoadPlace { dest, place } => {
            vec![(place.clone(), LoanAccess::Read, span_for(*dest))]
        }
        MirInstr::Store { place, src } => {
            vec![(place.clone(), LoanAccess::Write, span_for(*src))]
        }
        MirInstr::StoreRef { place, reference } => {
            vec![(place.clone(), LoanAccess::Write, span_for(*reference))]
        }
        MirInstr::MultiSet {
            receiver_place,
            arg_places,
            value_place,
            call,
            value,
            value_keyword,
            ..
        } => {
            let mut positional = arg_places.clone();
            let keyword = if *value_keyword {
                vec![value_place.clone()]
            } else {
                positional.push(value_place.clone());
                Vec::new()
            };
            subscript_accesses(call, receiver_place.as_ref(), &positional, &keyword, *value)
        }
        MirInstr::Index {
            dest,
            base_place,
            index_place,
            call,
            ..
        } => {
            if let Some(call) = call {
                subscript_accesses(
                    call,
                    base_place.as_ref(),
                    std::slice::from_ref(index_place),
                    &[],
                    *dest,
                )
            } else {
                base_place
                    .iter()
                    .chain(index_place.iter())
                    .cloned()
                    .map(|place| (place, LoanAccess::Read, span_for(*dest)))
                    .collect()
            }
        }
        MirInstr::Slice {
            dest,
            object_place,
            arg_places,
            call,
            ..
        }
        | MirInstr::MultiIndex {
            dest,
            object_place,
            arg_places,
            call,
            ..
        } => {
            if let Some(call) = call {
                subscript_accesses(call, object_place.as_ref(), arg_places, &[], *dest)
            } else {
                object_place
                    .iter()
                    .chain(arg_places.iter().flatten())
                    .cloned()
                    .map(|place| (place, LoanAccess::Read, span_for(*dest)))
                    .collect()
            }
        }
        MirInstr::VariantSet { place, value, .. } => {
            vec![(place.clone(), LoanAccess::Write, span_for(*value))]
        }
        MirInstr::VariantReplace { place, value, .. } => {
            vec![(place.clone(), LoanAccess::Write, span_for(*value))]
        }
        MirInstr::MovePlace { dest, place } => {
            vec![(place.clone(), LoanAccess::Write, span_for(*dest))]
        }
        MirInstr::MakeClosure { dest, captures, .. } => captures
            .iter()
            .map(|capture| {
                (
                    capture.place.clone(),
                    if capture.mode == MirCaptureMode::Move {
                        LoanAccess::Write
                    } else {
                        LoanAccess::Read
                    },
                    span_for(*dest),
                )
            })
            .collect(),
        MirInstr::ConsumePlace { place, marker } => {
            vec![(place.clone(), LoanAccess::Write, span_for(*marker))]
        }
        MirInstr::Call {
            dest,
            func,
            arg_places,
            kwarg_places,
            capture_accesses,
            ..
        } => {
            // Formatting intrinsics borrow their retained caller places only
            // to keep pointer-backed values alive through `write_to`. Ordinary
            // retained call places belong to `mut`/`ref` parameters and remain
            // exclusive writes.
            let access = if matches!(func.0.as_str(), "print" | "String" | "repr") {
                LoanAccess::Read
            } else {
                LoanAccess::Write
            };
            let mut accesses = arg_places
                .iter()
                .flatten()
                .chain(kwarg_places.iter().flatten())
                .cloned()
                .map(|place| (place, access, span_for(*dest)))
                .collect::<Vec<_>>();
            accesses.extend(
                capture_accesses
                    .iter()
                    .map(|access| captured(access, *dest)),
            );
            accesses
        }
        MirInstr::CallIndirect {
            dest,
            callee_place,
            arg_places,
            kwarg_places,
            capture_accesses,
            ..
        } => {
            let mut accesses = callee_place
                .iter()
                .chain(arg_places.iter().flatten())
                .chain(kwarg_places.iter().flatten())
                .cloned()
                .map(|place| (place, LoanAccess::Write, span_for(*dest)))
                .collect::<Vec<_>>();
            accesses.extend(
                capture_accesses
                    .iter()
                    .map(|access| captured(access, *dest)),
            );
            accesses
        }
        MirInstr::MethodCall {
            dest,
            recv_place,
            arg_places,
            kwarg_places,
            capture_accesses,
            ..
        } => {
            let mut accesses = recv_place
                .iter()
                .chain(arg_places.iter().flatten())
                .chain(kwarg_places.iter().flatten())
                .cloned()
                .map(|place| (place, LoanAccess::Write, span_for(*dest)))
                .collect::<Vec<_>>();
            accesses.extend(
                capture_accesses
                    .iter()
                    .map(|access| captured(access, *dest)),
            );
            accesses
        }
        MirInstr::DropVar { var } => {
            vec![(MirPlace::root(*var, None), LoanAccess::Write, fallback)]
        }
        _ => Vec::new(),
    }
}

fn loan_error(
    f: &MirFunction,
    place: &MirPlace,
    reference: VarId,
    span: crate::token::SourceSpan,
) -> OwnershipError {
    OwnershipError::LoanConflict {
        place: place_display(&f.var_names[place.root as usize], &place_path(place)),
        loan: f.var_names[reference as usize].clone(),
        span,
    }
}

/// A place's move/init state. A three-point lattice ordered by how "moved" a
/// place might be; the merge of disagreeing paths is `MaybeMoved`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Own {
    /// Initialized and not transferred — safe to use.
    Owned,
    /// Transferred (`^`) on every path to here — using it is a use-after-move.
    Moved,
    /// Transferred on some paths but not others — using it is a conditional move.
    MaybeMoved,
}

/// The dataflow join (least upper bound): equal states are preserved; any
/// disagreement between `Owned` and `Moved`, or anything involving `MaybeMoved`,
/// becomes `MaybeMoved`.
fn join(a: Own, b: Own) -> Own {
    match (a, b) {
        (Own::Owned, Own::Owned) => Own::Owned,
        (Own::Moved, Own::Moved) => Own::Moved,
        _ => Own::MaybeMoved,
    }
}

/// A total order on the lattice: `Moved(2) > MaybeMoved(1) > Owned(0)`.
fn severity(o: Own) -> u8 {
    match o {
        Own::Owned => 0,
        Own::MaybeMoved => 1,
        Own::Moved => 2,
    }
}

// --- Place-tree ownership lattice (field-sensitive partial moves) -----------

/// One projection step in a place path. Dynamic indices collapse to a wildcard
/// (`Index`) and overlap every constant index. Constant indices exist only for
/// compiler-private heterogeneous Tuple storage, where they let independently
/// owned elements retain distinct move state.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Key {
    Field(String),
    Index,
    ConstIndex(usize),
    Variant(usize),
}

fn keys_overlap(left: &Key, right: &Key) -> bool {
    left == right
        || matches!(
            (left, right),
            (Key::Index, Key::ConstIndex(_)) | (Key::ConstIndex(_), Key::Index)
        )
}

/// Map a MIR place's projection chain to a path of lattice keys.
fn place_path(place: &MirPlace) -> Vec<Key> {
    place
        .proj
        .iter()
        .map(|p| match p {
            Proj::Field(f) => Key::Field(f.clone()),
            Proj::Index(_) => Key::Index,
            Proj::ConstIndex(index) => Key::ConstIndex(*index),
            Proj::Variant(index) => Key::Variant(*index),
        })
        .collect()
}

/// A human-readable place name (`p`, `p.a`, `p.items[…]`) for diagnostics.
fn place_display(root: &str, path: &[Key]) -> String {
    let mut s = root.to_string();
    for k in path {
        match k {
            Key::Field(f) => {
                s.push('.');
                s.push_str(f);
            }
            Key::Index => s.push_str("[…]"),
            Key::ConstIndex(index) => {
                s.push('[');
                s.push_str(&index.to_string());
                s.push(']');
            }
            Key::Variant(index) => {
                s.push_str("[alternative#");
                s.push_str(&index.to_string());
                s.push(']');
            }
        }
    }
    s
}

/// The move/init state of a place *and everything under it*, as a tree. `base`
/// is the state of this node's own value and of any child not present in
/// `children`; `children` refine specific sub-places (fields / the wildcard
/// index). A partial move is `base = Owned` with a `Moved` child. Invariant: a
/// `base == Moved` node has no children (moving the whole clears sub-state); a
/// control-flow join may produce `base == MaybeMoved` with children.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Node {
    base: Own,
    children: BTreeMap<Key, Node>,
}

impl Node {
    fn owned() -> Node {
        Node {
            base: Own::Owned,
            children: BTreeMap::new(),
        }
    }

    /// Severity of *reading the whole subtree* at this node, paired with the
    /// relative path of the worst offender (for a precise diagnostic): the worst
    /// of its own base (path `[]`) and every descendant's whole severity — a
    /// moved child taints a whole read of the parent, and is named as the blame.
    fn whole(&self) -> (Own, Vec<Key>) {
        let mut worst = (self.base, Vec::new());
        for (k, c) in &self.children {
            let (sev, sub) = c.whole();
            if severity(sev) > severity(worst.0) {
                let mut full = vec![k.clone()];
                full.extend(sub);
                worst = (sev, full);
            }
        }
        worst
    }

    /// The state of *reading* the place reached by `path` (its whole subtree),
    /// combined with any moved ancestor passed through along the way. Returns the
    /// severity and the blamed sub-path: a moved *ancestor* blames the ancestor,
    /// a moved *descendant* of a whole read blames the descendant.
    fn read(&self, path: &[Key]) -> (Own, Vec<Key>) {
        match path.split_first() {
            None => self.whole(),
            // A moved ancestor on the way down blames the ancestor itself.
            Some(_) if self.base != Own::Owned => (self.base, Vec::new()),
            Some((key, rest)) => {
                let mut worst = (Own::Owned, Vec::new());
                for (candidate, child) in self
                    .children
                    .iter()
                    .filter(|(candidate, _)| keys_overlap(key, candidate))
                {
                    let (sev, subpath) = child.read(rest);
                    if severity(sev) > severity(worst.0) {
                        let mut full = vec![candidate.clone()];
                        full.extend(subpath);
                        worst = (sev, full);
                    }
                }
                worst
            }
        }
    }

    /// The base state of the *node itself* reached by `path` — only ancestor
    /// bases matter, not sibling/descendant moves. Used to check a field write,
    /// whose parent must merely be initialized (not wholly moved), so writing
    /// `p.a` is legal even when the sibling `p.b` has been moved out. Blames the
    /// nearest moved ancestor.
    fn base_at(&self, path: &[Key]) -> (Own, Vec<Key>) {
        match path.split_first() {
            None => (self.base, Vec::new()),
            Some((_, _)) if self.base != Own::Owned => (self.base, Vec::new()),
            Some((key, rest)) => {
                let mut worst = (Own::Owned, Vec::new());
                for (candidate, child) in self
                    .children
                    .iter()
                    .filter(|(candidate, _)| keys_overlap(key, candidate))
                {
                    let (sev, subpath) = child.base_at(rest);
                    if severity(sev) > severity(worst.0) {
                        let mut full = vec![candidate.clone()];
                        full.extend(subpath);
                        worst = (sev, full);
                    }
                }
                worst
            }
        }
    }

    /// Mark the place at `path` as wholly moved (clearing its sub-state).
    fn do_move(&mut self, path: &[Key]) {
        match path.split_first() {
            None => {
                *self = Node {
                    base: Own::Moved,
                    children: BTreeMap::new(),
                }
            }
            Some((k, rest)) => {
                let base = self.base;
                self.children
                    .entry(k.clone())
                    .or_insert_with(|| Node {
                        base,
                        children: BTreeMap::new(),
                    })
                    .do_move(rest);
            }
        }
    }

    /// Re-initialize the place at `path` to `Owned` (a def / field store).
    fn do_def(&mut self, path: &[Key]) {
        match path.split_first() {
            None => *self = Node::owned(),
            Some((k, rest)) => {
                // Reinitializing a field of a wholly-moved value is itself invalid
                // (caught as a write through a moved parent); don't corrupt state.
                if self.base == Own::Moved {
                    return;
                }
                let base = self.base;
                self.children
                    .entry(k.clone())
                    .or_insert_with(|| Node {
                        base,
                        children: BTreeMap::new(),
                    })
                    .do_def(rest);
            }
        }
    }
}

/// Join two place-trees at a control-flow merge (a per-node dataflow lub). A key
/// present on only one side inherits that side's `base` for the missing child.
fn join_node(a: &Node, b: &Node) -> Node {
    let base = join(a.base, b.base);
    let mut children = BTreeMap::new();
    let mut keys: Vec<&Key> = a.children.keys().chain(b.children.keys()).collect();
    keys.sort_unstable();
    keys.dedup();
    for k in keys {
        let ca = a.children.get(k).cloned().unwrap_or(Node {
            base: a.base,
            children: BTreeMap::new(),
        });
        let cb = b.children.get(k).cloned().unwrap_or(Node {
            base: b.base,
            children: BTreeMap::new(),
        });
        children.insert(k.clone(), join_node(&ca, &cb));
    }
    Node { base, children }
}

/// A basic block's successors (by terminator).
fn successors(term: &MirTerm) -> Vec<usize> {
    match term {
        MirTerm::Jump(t) => vec![*t],
        MirTerm::Branch { then_b, else_b, .. } => vec![*then_b, *else_b],
        // `EscapeJump` only appears inside a `try` region (never a function body),
        // so this — which walks function-body successors — never sees it; it leaves
        // this CFG like a `Return`.
        MirTerm::Return(_)
        | MirTerm::ReturnWithCleanup { .. }
        | MirTerm::FallOff
        | MirTerm::EscapeJump { .. } => vec![],
    }
}

/// How an instruction touches a place: a whole-value *read* (using the subtree),
/// or the *structural* parent-check of a field write (the parent must merely be
/// initialized, not wholly moved — so writing `p.a` is fine when `p.b` is moved).
enum Touch {
    Read,
    WriteParent,
}

/// The places an instruction *reads* or structurally touches (for reporting),
/// each with the register whose span points at the offending source. Moves and
/// definitions are applied separately by [`apply_effects`].
fn place_uses(i: &MirInstr) -> Vec<(VarId, Vec<Key>, Touch, Reg)> {
    match i {
        MirInstr::EstablishLoans { loans, marker, .. } => loans
            .iter()
            .map(|loan| {
                (
                    loan.place.root,
                    place_path(&loan.place),
                    Touch::Read,
                    *marker,
                )
            })
            .collect(),
        // A whole-variable read/borrow (a bare `x`) or move (`x^`): reads the
        // whole variable first.
        MirInstr::UseVar { dest, var, .. } => vec![(*var, Vec::new(), Touch::Read, *dest)],
        // A place read (`p.a`, a read-modify-write load) or a partial move
        // (`p.a^`): reads that specific sub-place.
        MirInstr::LoadPlace { dest, place } | MirInstr::MovePlace { dest, place } => {
            vec![(place.root, place_path(place), Touch::Read, *dest)]
        }
        MirInstr::ConsumePlace { place, marker } => {
            vec![(place.root, place_path(place), Touch::Read, *marker)]
        }
        MirInstr::MakeClosure { dest, captures, .. } => captures
            .iter()
            .map(|capture| {
                (
                    capture.place.root,
                    place_path(&capture.place),
                    Touch::Read,
                    *dest,
                )
            })
            .collect(),
        // A place write `p…​.f = e`: the *parent* place must be initialized (the
        // field itself is being overwritten, so it need not be). A statically
        // selected private Tuple element has the same independent-place
        // semantics. A dynamic-index write keeps the whole chain as the parent.
        MirInstr::Store { place, src } => {
            let mut path = place_path(place);
            if matches!(
                place.proj.last(),
                Some(Proj::Field(_) | Proj::ConstIndex(_))
            ) {
                path.pop(); // drop the final sub-place — check its parent
            }
            vec![(place.root, path, Touch::WriteParent, *src)]
        }
        MirInstr::StoreRef { place, reference } => {
            let mut path = place_path(place);
            if matches!(
                place.proj.last(),
                Some(Proj::Field(_) | Proj::ConstIndex(_))
            ) {
                path.pop();
            }
            vec![(place.root, path, Touch::WriteParent, *reference)]
        }
        MirInstr::MultiSet {
            receiver_place,
            value,
            ..
        } => receiver_place
            .iter()
            .map(|place| (place.root, place_path(place), Touch::Read, *value))
            .collect(),
        MirInstr::VariantSet { place, value, .. } => {
            let mut path = place_path(place);
            if matches!(place.proj.last(), Some(Proj::Field(_))) {
                path.pop();
            }
            vec![(place.root, path, Touch::WriteParent, *value)]
        }
        MirInstr::VariantReplace { place, value, .. } => {
            let mut path = place_path(place);
            if matches!(place.proj.last(), Some(Proj::Field(_))) {
                path.pop();
            }
            vec![(place.root, path, Touch::WriteParent, *value)]
        }
        // The `for` iterator variable is read (and advanced) — treat as a whole read.
        MirInstr::HasNext { dest, iter, .. }
        | MirInstr::Next { dest, iter, .. }
        | MirInstr::TryNext { dest, iter, .. } => {
            vec![(*iter, Vec::new(), Touch::Read, *dest)]
        }
        _ => Vec::new(),
    }
}

/// Apply an instruction's move/def effects to a place-tree state (no reporting):
/// a `DefVar` (re)initializes a whole variable, a `^` transfer moves one, a
/// partial move `p.a^` moves that sub-place, and a field store reinitializes the
/// written field.
fn apply_effects(state: &mut [Node], i: &MirInstr) {
    match i {
        MirInstr::DefVar { var, .. } => state[*var as usize].do_def(&[]),
        MirInstr::UseVar {
            var,
            mode: UseMode::Move,
            ..
        } => state[*var as usize].do_move(&[]),
        MirInstr::MakeClosure { captures, .. } => {
            for capture in captures {
                if capture.mode == MirCaptureMode::Move {
                    state[capture.place.root as usize].do_move(&place_path(&capture.place));
                }
            }
        }
        MirInstr::MovePlace { place, .. } => {
            state[place.root as usize].do_move(&place_path(place));
        }
        MirInstr::ConsumePlace { place, .. } => {
            state[place.root as usize].do_move(&place_path(place));
        }
        // A field or statically selected private Tuple-element store
        // reinitializes exactly that sub-place. A dynamic-index store cannot
        // precisely reinitialize one element, so it remains conservative.
        MirInstr::Store { place, .. }
        | MirInstr::StoreRef { place, .. }
        | MirInstr::VariantSet { place, .. }
        | MirInstr::VariantReplace { place, .. }
            if matches!(
                place.proj.last(),
                Some(Proj::Field(_) | Proj::ConstIndex(_))
            ) =>
        {
            state[place.root as usize].do_def(&place_path(place));
        }
        _ => {}
    }
}

/// Apply a block's instructions to a place-tree state, *without* reporting (used
/// to reach the dataflow fixpoint).
fn transfer(state: &mut [Node], instrs: &[MirInstr]) {
    for i in instrs {
        apply_effects(state, i);
    }
}

/// Join two per-variable place-tree states (control-flow merge).
fn join_states(a: &[Node], b: &[Node]) -> Vec<Node> {
    a.iter().zip(b).map(|(x, y)| join_node(x, y)).collect()
}

/// Analyze one function body for move violations, field-sensitively (partial
/// moves): a value transferred with `^` — whole (`x^`) or a field (`p.a^`) — may
/// not be read again on that path, but a disjoint sibling (`p.b`) stays usable.
fn analyze_moves(f: &MirFunction) -> Result<(), OwnershipError> {
    let nb = f.blocks.len();
    let nv = f.n_vars;

    // Predecessor lists, from each block's successors.
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); nb];
    for (b, blk) in f.blocks.iter().enumerate() {
        for s in successors(&blk.term) {
            preds[s].push(b);
        }
    }

    // The entry starts every variable `Owned` — the checker guarantees definite
    // assignment before use, so this never causes a false negative for our
    // purpose (tracking transfers) and avoids a spurious "uninitialized" lattice.
    let entry: Vec<Node> = vec![Node::owned(); nv];
    // `Owned` is a real program state, not the lattice bottom. Seeding every
    // block with it makes a loop header spuriously join a definite preheader
    // move with an as-yet-unvisited backedge and permanently report
    // `MaybeMoved`. Keep unreachable/unvisited states absent until a predecessor
    // supplies a fact instead.
    let mut in_states: Vec<Option<Vec<Node>>> = vec![None; nb];
    let mut out_states: Vec<Option<Vec<Node>>> = vec![None; nb];

    // Iterate to a fixpoint: in[b] = ⨆ out[pred], out[b] = transfer(in[b]).
    let mut changed = true;
    while changed {
        changed = false;
        #[allow(clippy::needless_range_loop)]
        for b in 0..nb {
            let new_in = if b == 0 || preds[b].is_empty() {
                entry.clone() // entry block, or an unreachable one
            } else {
                let mut predecessors = preds[b]
                    .iter()
                    .filter_map(|predecessor| out_states[*predecessor].as_ref());
                let Some(first) = predecessors.next() else {
                    continue;
                };
                let mut acc = first.clone();
                for predecessor in predecessors {
                    acc = join_states(&acc, predecessor);
                }
                acc
            };
            let mut new_out = new_in.clone();
            transfer(&mut new_out, &f.blocks[b].instrs);
            if in_states[b].as_ref() != Some(&new_in) || out_states[b].as_ref() != Some(&new_out) {
                in_states[b] = Some(new_in);
                out_states[b] = Some(new_out);
                changed = true;
            }
        }
    }

    // Reporting pass: replay each block from its fixed-point in-state, checking
    // every place use against the current move-state. Returns the first violation.
    #[allow(clippy::needless_range_loop)]
    for b in 0..nb {
        let mut state = in_states[b].clone().unwrap_or_else(|| entry.clone());
        for instr in &f.blocks[b].instrs {
            for (root, path, touch, reg) in place_uses(instr) {
                let node = &state[root as usize];
                let (sev, blame) = match touch {
                    Touch::Read => node.read(&path),
                    Touch::WriteParent => node.base_at(&path),
                };
                if sev != Own::Owned {
                    let span = f
                        .spans
                        .0
                        .get(&reg.0)
                        .map(|(s, _)| s.clone())
                        .unwrap_or_else(|| crate::token::SourceSpan::new(None, (0, 0)));
                    let var = place_display(&f.var_names[root as usize], &blame);
                    return Err(match sev {
                        Own::Moved => OwnershipError::UseAfterMove { var, span },
                        _ => OwnershipError::ConditionallyMoved { var, span },
                    });
                }
            }
            apply_effects(&mut state, instr);
        }
    }
    Ok(())
}

#[cfg(test)]
mod constant_tuple_place_tests {
    use super::*;

    fn place(projection: Proj) -> MirPlace {
        let mut place = MirPlace::root(0, None);
        place.proj.push(projection);
        place
    }

    #[test]
    fn constant_tuple_indices_are_disjoint_but_dynamic_indices_overlap_them() {
        let first = place(Proj::ConstIndex(0));
        let second = place(Proj::ConstIndex(1));
        let dynamic = place(Proj::Index(Reg(0)));
        assert!(!mir_places_overlap(&first, &second));
        assert!(mir_places_overlap(&first, &dynamic));
        assert!(mir_places_overlap(&dynamic, &second));
    }

    #[test]
    fn ownership_tracks_constant_tuple_elements_independently() {
        let mut state = Node::owned();
        state.do_move(&[Key::ConstIndex(0)]);
        assert_eq!(state.read(&[Key::ConstIndex(0)]).0, Own::Moved);
        assert_eq!(state.read(&[Key::ConstIndex(1)]).0, Own::Owned);
        assert_eq!(state.read(&[Key::Index]).0, Own::Moved);
    }

    #[test]
    fn self_is_never_droppable_even_when_its_stable_slot_is_not_leading() {
        let function = MirFunction {
            blocks: Vec::new(),
            n_regs: 0,
            n_vars: 3,
            var_names: vec!["argument".into(), "self".into(), "local".into()],
            n_params: 1,
            param_types: Vec::new(),
            owned_params: vec![false],
            deinit_params: vec![false],
            ref_params: vec![false],
            returns_reference: false,
            var_tys: HashMap::new(),
            ret_ty: None,
            raises: false,
            error_ty: None,
            spans: SpanTable::default(),
            reg_types: HashMap::new(),
        };

        assert!(!is_droppable_root(&function, 1));
        assert!(is_droppable_root(&function, 2));
    }
}

#[cfg(test)]
mod interior_origin_tests {
    use super::*;
    use crate::mir::MirLoan;
    use crate::origin::OriginSeg;
    use crate::token::SourceSpan;
    use crate::types::Ty;
    use std::collections::HashMap;

    fn origin(path: Vec<OriginSeg>) -> MirInteriorOrigin {
        MirInteriorOrigin { root: 0, path }
    }

    fn element_origin() -> MirInteriorOrigin {
        origin(vec![OriginSeg::Interior("element".into())])
    }

    fn loan(interior: Option<MirInteriorOrigin>) -> MirLoan {
        MirLoan {
            place: MirPlace::root(0, None),
            mutable: true,
            interior,
        }
    }

    fn establish(reference: VarId, marker: u32, interior: Option<MirInteriorOrigin>) -> MirInstr {
        MirInstr::EstablishLoans {
            reference,
            loans: vec![loan(interior)],
            marker: Reg(marker),
        }
    }

    fn invalidate(marker: u32) -> MirInstr {
        MirInstr::InvalidateInteriors {
            base: origin(Vec::new()),
            except: None,
            include_base_generation: false,
            marker: Reg(marker),
        }
    }

    fn use_reference(reference: VarId, dest: u32) -> MirInstr {
        MirInstr::UseVar {
            dest: Reg(dest),
            var: reference,
            mode: UseMode::BorrowShared,
        }
    }

    fn block(instrs: Vec<MirInstr>, term: MirTerm) -> MirBlock {
        MirBlock { instrs, term }
    }

    fn function(blocks: Vec<MirBlock>, spans: &[(u32, (usize, usize))]) -> MirFunction {
        let mut span_table = SpanTable::default();
        for (reg, span) in spans {
            span_table.0.insert(
                *reg,
                (SourceSpan::new(Some("interior.mojo".into()), *span), None),
            );
        }
        MirFunction {
            blocks,
            n_regs: 128,
            n_vars: 3,
            var_names: vec!["values".into(), "first".into(), "second".into()],
            n_params: 0,
            param_types: Vec::new(),
            owned_params: Vec::new(),
            deinit_params: Vec::new(),
            ref_params: Vec::new(),
            returns_reference: false,
            var_tys: HashMap::new(),
            ret_ty: Some(Ty::None),
            raises: false,
            error_ty: None,
            spans: span_table,
            reg_types: HashMap::new(),
        }
    }

    #[test]
    fn invalidation_reports_use_and_mutation_spans() {
        let f = function(
            vec![block(
                vec![
                    establish(1, 0, Some(element_origin())),
                    invalidate(1),
                    use_reference(1, 2),
                ],
                MirTerm::Return(None),
            )],
            &[(1, (10, 20)), (2, (30, 35))],
        );
        match analyze_interior_origins(&f) {
            Err(OwnershipError::InvalidatedInteriorReference {
                reference,
                origin,
                span,
                invalidated_at,
            }) => {
                assert_eq!(reference, "first");
                assert_eq!(origin, "values[\"element\"]");
                assert_eq!(span.span, (30, 35));
                assert_eq!(invalidated_at.span, (10, 20));
            }
            other => panic!("expected invalidated interior reference, got {other:?}"),
        }
    }

    #[test]
    fn invalidation_on_one_branch_rejects_use_after_join() {
        let f = function(
            vec![
                block(
                    vec![establish(1, 0, Some(element_origin()))],
                    MirTerm::Branch {
                        cond: Reg(99),
                        then_b: 1,
                        else_b: 2,
                    },
                ),
                block(vec![invalidate(1)], MirTerm::Jump(3)),
                block(Vec::new(), MirTerm::Jump(3)),
                block(vec![use_reference(1, 2)], MirTerm::Return(None)),
            ],
            &[(1, (10, 20)), (2, (30, 35))],
        );
        assert!(matches!(
            analyze_interior_origins(&f),
            Err(OwnershipError::InvalidatedInteriorReference { .. })
        ));
    }

    #[test]
    fn invalidation_on_loop_backedge_rejects_maybe_stale_exit() {
        let f = function(
            vec![
                block(
                    vec![establish(1, 0, Some(element_origin()))],
                    MirTerm::Jump(1),
                ),
                block(
                    Vec::new(),
                    MirTerm::Branch {
                        cond: Reg(99),
                        then_b: 2,
                        else_b: 3,
                    },
                ),
                block(vec![invalidate(1)], MirTerm::Jump(1)),
                block(vec![use_reference(1, 2)], MirTerm::Return(None)),
            ],
            &[(1, (10, 20)), (2, (30, 35))],
        );
        assert!(matches!(
            analyze_interior_origins(&f),
            Err(OwnershipError::InvalidatedInteriorReference { .. })
        ));
    }

    #[test]
    fn rebinding_installs_a_fresh_generation() {
        let f = function(
            vec![block(
                vec![
                    establish(1, 0, Some(element_origin())),
                    invalidate(1),
                    establish(1, 2, Some(element_origin())),
                    use_reference(1, 3),
                ],
                MirTerm::Return(None),
            )],
            &[],
        );
        assert!(analyze_interior_origins(&f).is_ok());
    }

    #[test]
    fn refresh_on_invalidating_branch_preserves_path_correlation() {
        let f = function(
            vec![
                block(
                    vec![establish(1, 0, Some(element_origin()))],
                    MirTerm::Branch {
                        cond: Reg(99),
                        then_b: 1,
                        else_b: 2,
                    },
                ),
                block(
                    vec![invalidate(1), establish(1, 2, Some(element_origin()))],
                    MirTerm::Jump(3),
                ),
                block(Vec::new(), MirTerm::Jump(3)),
                block(vec![use_reference(1, 3)], MirTerm::Return(None)),
            ],
            &[],
        );
        assert!(analyze_interior_origins(&f).is_ok());
    }

    #[test]
    fn overlapping_interior_loans_are_not_exclusive() {
        let f = function(
            vec![block(
                vec![
                    establish(1, 0, Some(element_origin())),
                    establish(2, 1, Some(element_origin())),
                    use_reference(1, 2),
                    use_reference(2, 3),
                ],
                MirTerm::Return(None),
            )],
            &[],
        );
        assert!(analyze_loans(&f).is_ok());
    }

    #[test]
    fn overlapping_ordinary_mutable_loans_still_conflict() {
        let f = function(
            vec![block(
                vec![
                    establish(1, 0, None),
                    establish(2, 1, None),
                    use_reference(1, 2),
                    use_reference(2, 3),
                ],
                MirTerm::Return(None),
            )],
            &[],
        );
        assert!(matches!(
            analyze_loans(&f),
            Err(OwnershipError::LoanConflict { .. })
        ));
    }

    #[test]
    fn stale_uses_inside_try_handlers_are_checked() {
        let try_instr = MirInstr::Try {
            body: vec![block(
                vec![invalidate(1), MirInstr::Raise { src: Reg(2) }],
                MirTerm::FallOff,
            )],
            handler: Some((
                None,
                vec![block(vec![use_reference(1, 3)], MirTerm::FallOff)],
            )),
            orelse: None,
            finalbody: None,
            cleanup: Vec::new(),
        };
        let f = function(
            vec![block(
                vec![establish(1, 0, Some(element_origin())), try_instr],
                MirTerm::Return(None),
            )],
            &[(1, (10, 20)), (3, (30, 35))],
        );
        assert!(matches!(
            analyze_interior_origins(&f),
            Err(OwnershipError::InvalidatedInteriorReference { .. })
        ));
    }

    #[test]
    fn invalidation_is_field_sensitive_and_targets_nested_interiors() {
        let left = origin(vec![
            OriginSeg::Field("left".into()),
            OriginSeg::Interior("element".into()),
        ]);
        let right = origin(vec![
            OriginSeg::Field("right".into()),
            OriginSeg::Interior("element".into()),
        ]);
        let left_base = origin(vec![OriginSeg::Field("left".into())]);
        assert!(interior_origin_invalidated_by(&left, &left_base, false));
        assert!(!interior_origin_invalidated_by(&right, &left_base, false));

        let nested = origin(vec![
            OriginSeg::Interior("element".into()),
            OriginSeg::Interior("element".into()),
        ]);
        assert!(!interior_origin_invalidated_by(
            &element_origin(),
            &element_origin(),
            false,
        ));
        assert!(interior_origin_invalidated_by(
            &element_origin(),
            &element_origin(),
            true,
        ));
        let projected = origin(vec![
            OriginSeg::Interior("element".into()),
            OriginSeg::Field("value".into()),
        ]);
        assert!(interior_origin_invalidated_by(
            &projected,
            &element_origin(),
            true,
        ));
        assert!(interior_origin_invalidated_by(
            &nested,
            &element_origin(),
            false,
        ));
        let sibling = origin(vec![OriginSeg::Interior("value".into())]);
        assert!(!interior_origin_invalidated_by(
            &sibling,
            &element_origin(),
            true,
        ));
    }
}

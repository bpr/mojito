//! Drop elaboration: inserting `DropVar`s on liveness edges, try-region
//! interior/escape cleanups, and drop-liveness transfer.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

/// Insert `DropVar`s at each variable's last use. A backward liveness dataflow
/// finds where each variable dies (touched, then dead), and a forward rebuild
/// splices a drop right after — skipping variables moved out with `^` (their new
/// owner drops them) so nothing is double-dropped. At a shared death point,
/// variables drop in reverse declaration order (descending `VarId`), except
/// that an `in_place` binding joins its producer's death point and its slot
/// drops first (see [`InPlaceDeaths`]). Values that
/// die on control-flow edges drop on the edge (splitting critical edges), and
/// `try` region interiors get the same per-instruction and edge elaboration
/// through [`elaborate_try_interior`], leaving cleanup lists only where MIR
/// has no block for the drop: a `try`'s raise edge, and a return or escape
/// crossing a `finally`.
#[allow(clippy::too_many_lines, reason = "TODO: split this pass")]
pub(super) fn elaborate_drops(
    function: MirFunction,
    value_parameters: &HashSet<VarId>,
    declarations: &MirDeclarations,
) -> MirFunction {
    let passing = ArgumentPassing::new(declarations);
    let f = &function;
    let nb = f.blocks.len();
    // Function-wide def-site counts — `DefVar`s and `DeclareVar`s, deep
    // through `try` regions: the reference that region-locality — and
    // therefore every `try` cleanup — is judged against. Inserted `DropVar`s
    // add no def sites, so the counts stay valid for the rebuilt blocks below.
    let function_defs = {
        let mut counts = HashMap::new();
        let (mut order, mut moved) = (Vec::new(), HashSet::new());
        collect_region_defs(&f.blocks, &mut counts, &mut order, &mut moved);
        counts
    };
    let loan_roots = drop_loan_generations(f);
    let generation_dests = loan_generation_dests(f);
    let generation_entries = loan_generation_entries_over(
        &f.blocks,
        &LoanGenerationState::default(),
        &generation_dests,
    );
    let RegisterLoans {
        uses: register_loan_uses,
        before_try: register_loan_before_try,
    } = register_loan_uses_over(
        &f.blocks,
        &generation_entries,
        &RegisterLoanState::default(),
        &loan_roots,
        &generation_dests,
        &f.reg_types,
        &passing,
    );
    let generation_exits: Vec<LoanGenerationState> = f
        .blocks
        .iter()
        .zip(&generation_entries)
        .map(|(block, entry)| {
            let mut state = entry.clone();
            for instruction in &block.instrs {
                transfer_loan_generation(&mut state, instruction, &generation_dests);
            }
            state
        })
        .collect();

    // First compute ordinary variable liveness, with each instruction's
    // register-loan uses folded in at their exact consuming point (so a
    // register loaded in one block and consumed in another keeps its owner
    // live across the edge), but without folding loan generation owners into
    // the sets. Generation retention is joined at CFG merges, so adding it
    // here would lose path correlation and make every historical owner of a
    // rebound aggregate appear live forever; `effective_drop_liveness` adds
    // it per edge instead.
    let predecessors = block_predecessors(&f.blocks);
    let mut live_in: Vec<HashSet<VarId>> = vec![HashSet::new(); nb];
    let mut worklist = Worklist::backward(&f.blocks);
    while let Some(b) = worklist.pop() {
        let live_out = block_live_out(f, b, &live_in);
        let new_in = transfer_drop_liveness(
            &f.blocks[b].instrs,
            &register_loan_uses[b],
            live_out,
            &live_in,
            &function_defs,
        );
        if new_in != live_in[b] {
            live_in[b] = new_in;
            for predecessor in &predecessors[b] {
                worklist.push(*predecessor);
            }
        }
    }

    let effective_live_in: Vec<HashSet<VarId>> = live_in
        .iter()
        .zip(&generation_entries)
        .map(|(live, generations)| effective_drop_liveness(live.clone(), generations, &loan_roots))
        .collect();
    let mut parameter_slots = comptime_binder_slots(&f.blocks);
    parameter_slots.extend(value_parameters);
    // The replay moves each instruction into its elaborated block.
    let mut function = function;
    let block_instrs: Vec<Vec<MirInstr>> = function
        .blocks
        .iter_mut()
        .map(|block| std::mem::take(&mut block.instrs))
        .collect();
    let f = &function;
    let droppable = |v: VarId| is_droppable_root(f, v) && !parameter_slots.contains(&v);
    let region_ctx = RegionDropCtx {
        f,
        parameter_slots: &parameter_slots,
        loan_roots: &loan_roots,
        generation_dests: &generation_dests,
        effective_live_in: &effective_live_in,
        passing: &passing,
        function_defs: &function_defs,
    };

    // (1) Block-internal drops: replay each block, tracking the live set after each
    // instruction, and drop the variables that die at their last use in-block.
    let mut blocks = Vec::with_capacity(nb);
    for (b, instrs) in block_instrs.into_iter().enumerate() {
        let empty_exit = instrs.is_empty() && terminator_targets(&f.blocks[b].term).is_empty();
        let replay = BlockReplay {
            instrs,
            register_uses: &register_loan_uses[b],
            deinit_params: &f.deinit_params,
            generation_entry: &generation_entries[b],
            loan_roots: &loan_roots,
            generation_dests: &generation_dests,
            function_defs: &function_defs,
        };
        let mut new_instrs = replay.elaborate(
            block_live_out(f, b, &live_in),
            |instr, live| live.extend(escape_live_in(instr, &live_in)),
            droppable,
            |point, cloned| {
                let instr = point.instr;
                // Fill the regions' escape-edge cleanups: the *outer*
                // variables live entering the `try` that are dead at the
                // escape target (so they die on the hidden `break`/`continue`
                // edge), minus the `try`'s own body-locals and moved-out
                // values.
                let (mut rdef, mut rmov) = (HashSet::new(), HashSet::new());
                try_region_defs(instr, &function_defs, &mut rdef, &mut rmov);
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
                let base: HashSet<VarId> = point
                    .live_before
                    .iter()
                    .copied()
                    .filter(|v| {
                        droppable(*v)
                            && !rdef.contains(v)
                            && !rmov.contains(v)
                            && !fin_used.contains(v)
                    })
                    .collect();
                fill_escape_cleanups(cloned, &base, &effective_live_in);
                seed_raise_cleanup(
                    cloned,
                    point,
                    &HashSet::new(),
                    &function_defs,
                    &effective_live_in,
                    droppable,
                );
                // Elaborate per-instruction/edge drops inside the regions,
                // seeded from this walk's liveness at the `try`. Nothing runs
                // after a top-level return, so its channel holds nothing.
                let none = HashSet::new();
                let interior = elaborate_try_interior(
                    &region_ctx,
                    cloned,
                    point.ordinary_after,
                    &Enclosing {
                        raise: &none,
                        return_live: &none,
                        return_ended: &none,
                    },
                    point.generation_before,
                    &register_loan_before_try[&(b, point.index)],
                );
                // Entry-edge deaths: a value live into the `try` that no
                // region path can observe (an unconditional silent rebind
                // precedes every potential raise) dies immediately before it.
                let entry_dead = point
                    .live_before
                    .iter()
                    .copied()
                    .filter(|v| {
                        droppable(*v) && !rmov.contains(v) && !interior.entry_live.contains(v)
                    })
                    .collect();
                (entry_dead, interior.raise_held)
            },
            None,
        );
        if empty_exit {
            // A CFG may funnel live reference-bearing aggregates into an empty
            // return block. There is no final instruction to host the normal
            // before/after generation death, so retire those owners immediately
            // before the terminator.
            let dying =
                effective_drop_liveness(HashSet::new(), &generation_entries[b], &loan_roots)
                    .into_iter()
                    .filter(|owner| droppable(*owner))
                    .collect();
            append_drops(&mut new_instrs, dying);
        }
        blocks.push(MirBlock {
            instrs: new_instrs,
            term: f.blocks[b].term.clone(),
        });
    }
    // (1b) Entry deaths: a consuming parameter the body never uses is dead on
    // arrival — Mojo destroys an unused `deinit self`'s fields before the
    // first statement runs — so it drops at the function's entry.
    if nb > 0 {
        let entry_live =
            effective_drop_liveness(live_in[0].clone(), &generation_entries[0], &loan_roots);
        let entry_dead: Vec<VarId> = (0..f.n_params)
            .map(|v| v as VarId)
            .filter(|v| droppable(*v) && !entry_live.contains(v))
            .collect();
        prepend_drops(&mut blocks[0].instrs, entry_dead);
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
        let mut succs: Vec<usize> = terminator_targets(&f.blocks[p].term);
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
                .filter(|&v| !live_on_edge.contains(&v) && droppable(v))
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

    // A top-level return crosses no `finally`: the keep-alives and their
    // spliced drops end what it held, so its terminator lists nothing.
    for block in &mut blocks {
        if let MirTerm::ReturnWithCleanup { value, .. } = &mut block.term {
            block.term = MirTerm::Return(value.take());
        }
    }

    // A `deinit` parameter is *consumed*, not destroyed: the spliced teardown
    // for it must skip the value's whole-value `__deinit__` (its resources already
    // moved into the receiver) while still destroying any residual fields. Drop
    // elaboration emits an ordinary `DropVar`; rewrite those to `ConsumeVar` for
    // deinit parameters. Their `^`-transferred fields are `Value::Moved` and a
    // no-op to drop, so this cannot double-free.
    if f.deinit_params.iter().any(|&d| d) {
        let is_deinit_param = |var: VarId| {
            (var as usize) < f.n_params
                && f.deinit_params.get(var as usize).copied().unwrap_or(false)
        };
        for_each_instr_deep_mut(&mut blocks, &mut |instr| {
            if let MirInstr::DropVar { var } = instr
                && is_deinit_param(*var)
            {
                *instr = MirInstr::ConsumeVar { var: *var };
            }
        });
    }

    function.blocks = blocks;
    function
}

/// Shared analysis inputs for elaborating drops inside `try` region mini-CFGs.
pub(super) struct RegionDropCtx<'a> {
    f: &'a MirFunction,
    /// The function's compile-time parameter slots: its `comptime for`
    /// binders ([`comptime_binder_slots`]) and its materialized value
    /// parameters (`materialized_parameter_slots`).
    parameter_slots: &'a HashSet<VarId>,
    /// Owner roots per loan generation, collected deep through regions.
    loan_roots: &'a BTreeMap<u32, DropLoanGeneration>,
    /// Destination domain per generation marker, collected deep.
    generation_dests: &'a BTreeMap<u32, Option<MirInteriorOrigin>>,
    /// Top-level effective live-in sets, bounding `EscapeJump` targets.
    effective_live_in: &'a [HashSet<VarId>],
    /// Which operands each instruction receives by value.
    passing: &'a ArgumentPassing<'a>,
    /// Function-wide `DefVar` counts, judging each `try` body's locals.
    function_defs: &'a HashMap<VarId, usize>,
}

impl RegionDropCtx<'_> {
    /// Whether the region's function drops the value in `v`
    /// ([`is_droppable_root`]); a compile-time parameter's slot — a `comptime
    /// for` binder or a value parameter — never.
    fn droppable(&self, v: VarId) -> bool {
        is_droppable_root(self.f, v) && !self.parameter_slots.contains(&v)
    }
}

/// Live-out seeds for one region's mini-CFG, per exit kind.
pub(super) struct RegionSeeds<'a> {
    /// Live on the normal `FallOff` continuation.
    fall_off: &'a HashSet<VarId>,
    /// Live at every potentially-raising instruction: what the raise edge's
    /// observer (handler, `finally`, or an enclosing handler) may still read.
    raise: &'a HashSet<VarId>,
    /// Live through `EscapeJump` exits — the `finally` still runs after
    /// those edges leave the region.
    finally_live: &'a HashSet<VarId>,
    /// Live through `Return`/`ReturnWithCleanup` exits: what the `finally`
    /// blocks a return crosses, and those beyond them, still read.
    return_live: &'a HashSet<VarId>,
    /// The slots of `return_live` those crossed `finally` blocks end; a
    /// return's cleanup list destroys the rest after the last of them.
    return_ended: &'a HashSet<VarId>,
    /// Top-level effective live-in sets, bounding `EscapeJump` targets.
    effective_live_in: &'a [HashSet<VarId>],
}

/// The channels of the region holding a `try`, as its interior sees them.
pub(super) struct Enclosing<'a> {
    /// What the enclosing raise observer reads ([`RegionSeeds::raise`]).
    raise: &'a HashSet<VarId>,
    /// [`RegionSeeds::return_live`] of the enclosing region.
    return_live: &'a HashSet<VarId>,
    /// [`RegionSeeds::return_ended`] of the enclosing region.
    return_ended: &'a HashSet<VarId>,
}

/// What a `try`'s interior elaboration reports to the walk holding it.
pub(super) struct TryInterior {
    /// The body region's effective entry liveness.
    entry_live: HashSet<VarId>,
    /// The slots a raise leaving the `try` still holds, once its own
    /// raise-edge cleanup and `finally` have run.
    raise_held: HashSet<VarId>,
}

/// Elaborate per-instruction and edge drops inside all four regions of one
/// `Try` (in place), seeding each region's liveness from the enclosing walk's
/// state at the instruction, and set the `try`'s raise-edge cleanup to the
/// candidates a raise in the body still holds. Returns the body's entry
/// liveness, so the caller can drop values no region path observes
/// immediately before the `try`, and what a raise leaving the `try` holds.
pub(super) fn elaborate_try_interior(
    ctx: &RegionDropCtx,
    try_instr: &mut MirInstr,
    after: &HashSet<VarId>,
    enclosing: &Enclosing,
    entry_generations: &LoanGenerationState,
    entry_registers: &RegisterLoanState,
) -> TryInterior {
    let MirInstr::Try {
        body,
        handler,
        orelse,
        finalbody,
        cleanup,
    } = try_instr
    else {
        return TryInterior {
            entry_live: HashSet::new(),
            raise_held: HashSet::new(),
        };
    };

    // The seeded outer candidates are never held at the handler's entry:
    // the raise edge ends those a raise holds, and no raise holds the rest.
    // So no escape out of the handler destroys them again.
    if let Some((_, h)) = handler {
        strip_escape_cleanups(h, cleanup);
    }

    // Handler/`else`/`finally` run after an arbitrary body prefix, so their
    // loan pictures are the union of every state the body can reach — more
    // retention than any single path, which at worst delays a drop to the
    // cleanup backstops. No SSA handle survives into a region entered by raise
    // or completion (each statement writes its registers before reading them),
    // so those regions' register-loan entries are empty.
    let body_any = region_any_generation_state(body, entry_generations, ctx.generation_dests);
    let handler_any = handler
        .as_ref()
        .map(|(_, h)| region_any_generation_state(h, &body_any, ctx.generation_dests));
    let orelse_any = orelse
        .as_ref()
        .map(|e| region_any_generation_state(e, &body_any, ctx.generation_dests));

    // Regions elaborate in dependency order: `finally` first (its entry
    // liveness seeds every other exit), then handler/`else`, then the body.
    // A return crossing the `finally` holds what the `finally` reads on the
    // return channel, less what the `finally` ends on the way through.
    let mut return_ended = enclosing.return_ended.clone();
    let (fin_live, return_live, fin_held, fin_ended) = if let Some(fb) = finalbody {
        let mut fin_entry = body_any.clone();
        if let Some(state) = &handler_any {
            fin_entry = join_loan_generation_states(fin_entry, state);
        }
        if let Some(state) = &orelse_any {
            fin_entry = join_loan_generation_states(fin_entry, state);
        }
        let return_seeds = RegionSeeds {
            fall_off: enclosing.return_live,
            raise: enclosing.raise,
            finally_live: enclosing.return_live,
            return_live: enclosing.return_live,
            return_ended: enclosing.return_ended,
            effective_live_in: ctx.effective_live_in,
        };
        let return_live = region_live_in(
            ctx,
            fb,
            &return_seeds,
            &fin_entry,
            &RegisterLoanState::default(),
        );
        let seeds = RegionSeeds {
            fall_off: after,
            raise: enclosing.raise,
            finally_live: after,
            return_live: enclosing.return_live,
            return_ended: enclosing.return_ended,
            effective_live_in: ctx.effective_live_in,
        };
        let (fin_live, fin_held) =
            elaborate_region_drops(ctx, fb, &seeds, &fin_entry, &RegisterLoanState::default());
        // The `finally` ends, on every path through it, what it holds live
        // that its fall-off does not need.
        let fin_ended: HashSet<VarId> = fin_live
            .iter()
            .copied()
            .filter(|v| !after.contains(v) && ctx.droppable(*v))
            .collect();
        return_ended.extend(fin_ended.iter().copied());
        (fin_live, return_live, fin_held, fin_ended)
    } else {
        (
            after.clone(),
            enclosing.return_live.clone(),
            HashSet::new(),
            HashSet::new(),
        )
    };

    // A raise inside the handler or `else` is not caught by this `try`; it
    // runs the `finally` and then propagates to the enclosing observer.
    let mut outward_raise = fin_live.clone();
    outward_raise.extend(enclosing.raise.iter().copied());

    let handler_live = handler.as_mut().map(|(binding, h)| {
        let seeds = RegionSeeds {
            fall_off: &fin_live,
            raise: &outward_raise,
            finally_live: &fin_live,
            return_live: &return_live,
            return_ended: &return_ended,
            effective_live_in: ctx.effective_live_in,
        };
        let (mut live, held) = elaborate_region_drops(
            ctx,
            h,
            &seeds,
            handler_any.as_ref().unwrap_or(&body_any),
            &RegisterLoanState::default(),
        );
        // The caught error arrives as the handler entry's block argument, so
        // the binding's pre-raise content is never observable, and an unread
        // binding dies on arrival.
        if let Some(bound) = binding
            && !live.remove(bound)
            && ctx.droppable(*bound)
            && let Some(entry) = h.first_mut()
        {
            entry.instrs.insert(0, MirInstr::DropVar { var: *bound });
        }
        (live, held)
    });
    let orelse_live = orelse.as_mut().map(|e| {
        let seeds = RegionSeeds {
            fall_off: &fin_live,
            raise: &outward_raise,
            finally_live: &fin_live,
            return_live: &return_live,
            return_ended: &return_ended,
            effective_live_in: ctx.effective_live_in,
        };
        elaborate_region_drops(
            ctx,
            e,
            &seeds,
            orelse_any.as_ref().unwrap_or(&body_any),
            &RegisterLoanState::default(),
        )
    });

    // Body: a raise lands in the handler when there is one (it catches every
    // error), otherwise it runs the `finally` and propagates outward. Normal
    // completion continues into `else` when present.
    let (body_raise, handler_held) =
        handler_live.map_or((outward_raise, None), |(live, held)| (live, Some(held)));
    let (body_fall_off, orelse_held) = orelse_live.map_or_else(
        || (fin_live.clone(), None),
        |(live, held)| (live, Some(held)),
    );
    let seeds = RegionSeeds {
        fall_off: &body_fall_off,
        raise: &body_raise,
        finally_live: &fin_live,
        return_live: &return_live,
        return_ended: &return_ended,
        effective_live_in: ctx.effective_live_in,
    };
    let (entry_live, body_held) =
        elaborate_region_drops(ctx, body, &seeds, entry_generations, entry_registers);

    // The raise edge destroys the body's locals and the seeded outer values
    // a raise in the body still holds; nothing else runs it.
    let mut listed = region_cleanup_vars(body, ctx.function_defs);
    for v in cleanup.drain(..) {
        if !listed.contains(&v) {
            listed.push(v);
        }
    }
    listed.retain(|v| body_held.contains(v) && ctx.droppable(*v));
    listed.sort_unstable_by(|a, b| b.cmp(a));
    *cleanup = listed;

    // What a raise leaving the `try` holds: an uncaught body raise past the
    // raise edge, a raise out of the handler or `else`, each past what the
    // `finally` ends, and a raise out of the `finally` itself.
    let mut raise_held: HashSet<VarId> = match &handler_held {
        Some(_) => HashSet::new(),
        None => body_held
            .into_iter()
            .filter(|v| !cleanup.contains(v))
            .collect(),
    };
    raise_held.extend(handler_held.into_iter().flatten());
    raise_held.extend(orelse_held.into_iter().flatten());
    raise_held.retain(|v| !fin_ended.contains(v));
    raise_held.extend(fin_held);
    TryInterior {
        entry_live,
        raise_held,
    }
}

/// A region's effective entry liveness under `seeds`, its blocks left
/// untouched: the liveness half of [`elaborate_region_drops`].
pub(super) fn region_live_in(
    ctx: &RegionDropCtx,
    blocks: &[MirBlock],
    seeds: &RegionSeeds,
    entry_generations: &LoanGenerationState,
    entry_registers: &RegisterLoanState,
) -> HashSet<VarId> {
    let Some(liveness) = region_liveness(ctx, blocks, seeds, entry_generations, entry_registers)
    else {
        return seeds.fall_off.clone();
    };
    effective_drop_liveness(
        liveness.live_in[0].clone(),
        &liveness.generation_entries[0],
        ctx.loan_roots,
    )
}

/// Run the ordinary death/`DropVar` elaboration over one region's mini-CFG —
/// the same backward liveness, loan-aware death rule, and edge splitting the
/// function's top-level blocks get — with live-outs seeded per exit kind and
/// the raise seed applied at every potentially-raising instruction, then
/// list on each return what it holds past the `finally` blocks it crosses.
/// Returns the region's effective entry liveness and the slots some raise
/// leaving the region still holds.
pub(super) fn elaborate_region_drops(
    ctx: &RegionDropCtx,
    blocks: &mut Vec<MirBlock>,
    seeds: &RegionSeeds,
    entry_generations: &LoanGenerationState,
    entry_registers: &RegisterLoanState,
) -> (HashSet<VarId>, HashSet<VarId>) {
    let f = ctx.f;
    let nb = blocks.len();
    let Some(RegionLiveness {
        generation_entries,
        registers:
            RegisterLoans {
                uses: register_uses,
                before_try: register_before_try,
            },
        live_in,
    }) = region_liveness(ctx, blocks, seeds, entry_generations, entry_registers)
    else {
        return (seeds.fall_off.clone(), HashSet::new());
    };

    let generation_exits: Vec<LoanGenerationState> = blocks
        .iter()
        .zip(&generation_entries)
        .map(|(block, entry)| {
            let mut state = entry.clone();
            for instruction in &block.instrs {
                transfer_loan_generation(&mut state, instruction, ctx.generation_dests);
            }
            state
        })
        .collect();

    let effective_entry =
        effective_drop_liveness(live_in[0].clone(), &generation_entries[0], ctx.loan_roots);

    // Block-internal drops: the top-level rebuild, over the region CFG.
    let mut raise_held = DenseSet::default();
    for b in 0..nb {
        let live_out = region_block_live_out(blocks, b, &live_in, seeds);
        let instrs = std::mem::take(&mut blocks[b].instrs);
        let replay = BlockReplay {
            instrs,
            register_uses: &register_uses[b],
            deinit_params: &f.deinit_params,
            generation_entry: &generation_entries[b],
            loan_roots: ctx.loan_roots,
            generation_dests: ctx.generation_dests,
            function_defs: ctx.function_defs,
        };
        blocks[b].instrs = replay.elaborate(
            live_out,
            |instr, live| {
                live.extend(escape_live_in(instr, seeds.effective_live_in));
                if raises_to_handler(instr) {
                    live.extend(seeds.raise.iter().copied());
                }
            },
            |v| ctx.droppable(v),
            |point, cloned| {
                let observed = seeds
                    .raise
                    .union(seeds.finally_live)
                    .copied()
                    .collect::<HashSet<_>>();
                seed_raise_cleanup(
                    cloned,
                    point,
                    &observed,
                    ctx.function_defs,
                    seeds.effective_live_in,
                    |v| ctx.droppable(v),
                );
                let nested = elaborate_try_interior(
                    ctx,
                    cloned,
                    point.ordinary_after,
                    &Enclosing {
                        raise: seeds.raise,
                        return_live: seeds.return_live,
                        return_ended: seeds.return_ended,
                    },
                    point.generation_before,
                    &register_before_try[&(b, point.index)],
                );
                let moved = try_moved_vars(point.instr);
                let entry_dead = point
                    .live_before
                    .iter()
                    .copied()
                    .filter(|v| {
                        ctx.droppable(*v) && !moved.contains(v) && !nested.entry_live.contains(v)
                    })
                    .collect();
                (entry_dead, nested.raise_held)
            },
            Some(&mut raise_held),
        );
    }

    // Region-internal edge drops with critical-edge splitting. Exit
    // terminators have no local successors, so only `Jump`/`Branch` edges are
    // processed and `rewire_target` covers every case.
    let mut pred_count = vec![0usize; nb];
    for block in blocks.iter().take(nb) {
        for s in terminator_targets(&block.term) {
            pred_count[s] += 1;
        }
    }
    for p in 0..nb {
        let mut succs: Vec<usize> = terminator_targets(&blocks[p].term);
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
                        ctx.loan_roots,
                    ),
                )
            })
            .collect();
        let live_out_p: HashSet<VarId> = edge_live
            .iter()
            .flat_map(|(_, live)| live.iter().copied())
            .collect();
        for &s in &succs {
            let live_on_edge = edge_live
                .iter()
                .find_map(|(successor, live)| (*successor == s).then_some(live))
                .expect("each successor has an effective edge-live set");
            let dying: Vec<VarId> = live_out_p
                .iter()
                .copied()
                .filter(|&v| !live_on_edge.contains(&v) && ctx.droppable(v))
                .collect();
            if dying.is_empty() {
                continue;
            }
            if n_succ == 1 {
                append_drops(&mut blocks[p].instrs, dying);
            } else if pred_count[s] == 1 {
                prepend_drops(&mut blocks[s].instrs, dying);
            } else {
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

    // Each return holds its live-out once the `finally` blocks it crosses
    // have run; it destroys what those `finally` blocks leave.
    let mut held: Vec<VarId> = seeds
        .return_live
        .iter()
        .copied()
        .filter(|v| ctx.droppable(*v) && !seeds.return_ended.contains(v))
        .collect();
    held.sort_unstable_by(|a, b| b.cmp(a));
    for block in blocks.iter_mut() {
        if let MirTerm::Return(value) | MirTerm::ReturnWithCleanup { value, .. } = &mut block.term {
            let value = value.take();
            block.term = if held.is_empty() {
                MirTerm::Return(value)
            } else {
                MirTerm::ReturnWithCleanup {
                    value,
                    cleanup: held.clone(),
                }
            };
        }
    }

    (effective_entry, raise_held.iter().collect())
}

/// One region's drop liveness: the loan generations entering each block,
/// the register-loan uses of each instruction, and each block's live-in.
struct RegionLiveness {
    generation_entries: Vec<LoanGenerationState>,
    registers: RegisterLoans,
    live_in: Vec<HashSet<VarId>>,
}

/// The backward drop liveness of a region's mini-CFG under `seeds`; `None`
/// for a region with no blocks.
fn region_liveness(
    ctx: &RegionDropCtx,
    blocks: &[MirBlock],
    seeds: &RegionSeeds,
    entry_generations: &LoanGenerationState,
    entry_registers: &RegisterLoanState,
) -> Option<RegionLiveness> {
    if blocks.is_empty() {
        return None;
    }
    let generation_entries =
        loan_generation_entries_over(blocks, entry_generations, ctx.generation_dests);
    let registers = register_loan_uses_over(
        blocks,
        &generation_entries,
        entry_registers,
        ctx.loan_roots,
        ctx.generation_dests,
        &ctx.f.reg_types,
        ctx.passing,
    );
    let predecessors = block_predecessors(blocks);
    let mut live_in: Vec<HashSet<VarId>> = vec![HashSet::new(); blocks.len()];
    let mut worklist = Worklist::backward(blocks);
    while let Some(b) = worklist.pop() {
        let live_out = region_block_live_out(blocks, b, &live_in, seeds);
        let new_in = transfer_region_drop_liveness(
            &blocks[b].instrs,
            &registers.uses[b],
            live_out,
            seeds,
            ctx.function_defs,
        );
        if new_in != live_in[b] {
            live_in[b] = new_in;
            for predecessor in &predecessors[b] {
                worklist.push(*predecessor);
            }
        }
    }
    Some(RegionLiveness {
        generation_entries,
        registers,
        live_in,
    })
}

/// A region block's live-out, per terminator kind: local `Jump`/`Branch`
/// successors read region liveness; each exit edge reads its seed.
pub(super) fn region_block_live_out(
    blocks: &[MirBlock],
    b: usize,
    live_in: &[HashSet<VarId>],
    seeds: &RegionSeeds,
) -> HashSet<VarId> {
    match &blocks[b].term {
        MirTerm::Jump(_)
        | MirTerm::Branch { .. }
        | MirTerm::ComptimeBranch { .. }
        | MirTerm::ComptimeFor { .. } => {
            let mut out = HashSet::new();
            for s in terminator_targets(&blocks[b].term) {
                if let Some(live) = live_in.get(s) {
                    out.extend(live);
                }
            }
            out
        }
        MirTerm::FallOff => seeds.fall_off.clone(),
        MirTerm::Return(_) | MirTerm::ReturnWithCleanup { .. } => seeds.return_live.clone(),
        // Region terminators are lowered before this analysis runs.
        MirTerm::Yield(_)
        | MirTerm::ElifCond(_)
        | MirTerm::Break { .. }
        | MirTerm::Continue { .. }
        | MirTerm::ComptimeYield(_)
        | MirTerm::ComptimeBreak
        | MirTerm::ComptimeContinue
        | MirTerm::Unreachable => HashSet::new(),
        MirTerm::EscapeJump { target, cleanup } => {
            let mut out = seeds.finally_live.clone();
            out.extend(cleanup.iter().copied());
            if let Some(live) = seeds.effective_live_in.get(*target) {
                out.extend(live);
            }
            out
        }
    }
}

/// Backward liveness transfer over a region block: `transfer_drop_liveness`
/// plus the raise seed at every potentially-raising instruction, so a value
/// the raise edge's observer may read is never vacated before a raise can
/// reach it.
pub(super) fn transfer_region_drop_liveness(
    instrs: &[MirInstr],
    register_uses: &[RegisterUses],
    mut live: HashSet<VarId>,
    seeds: &RegionSeeds,
    function_defs: &HashMap<VarId, usize>,
) -> HashSet<VarId> {
    for (instr, uses) in instrs.iter().zip(register_uses).rev() {
        transfer_instruction_drop_liveness(&mut live, instr, uses, function_defs);
        live.extend(escape_live_in(instr, seeds.effective_live_in));
        if raises_to_handler(instr) {
            live.extend(seeds.raise.iter().copied());
        }
    }
    live
}

/// The `^`-moved variables of a `try`'s regions (deep), excluded from the
/// entry-edge drop set (their values transferred to a new owner).
pub(super) fn try_moved_vars(try_instr: &MirInstr) -> HashSet<VarId> {
    let mut counts = HashMap::new();
    let (mut order, mut moved) = (Vec::new(), HashSet::new());
    collect_try_defs(try_instr, &mut counts, &mut order, &mut moved);
    moved
}

/// The union of every loan-generation state reachable anywhere in a region
/// (deep through nested `try`s) given its entry state — the conservative loan
/// picture for a region entered after an arbitrary prefix of this one.
pub(super) fn region_any_generation_state(
    blocks: &[MirBlock],
    entry: &LoanGenerationState,
    generation_dests: &BTreeMap<u32, Option<MirInteriorOrigin>>,
) -> LoanGenerationState {
    let entries = loan_generation_entries_over(blocks, entry, generation_dests);
    let mut any = entry.clone();
    for (block, block_entry) in blocks.iter().zip(&entries) {
        let mut state = block_entry.clone();
        any = join_loan_generation_states(any, block_entry);
        for instr in &block.instrs {
            if let MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } = instr
            {
                let mut sub = region_any_generation_state(body, &state, generation_dests);
                if let Some((_, h)) = handler {
                    let inner = region_any_generation_state(h, &sub, generation_dests);
                    sub = join_loan_generation_states(sub, &inner);
                }
                if let Some(e) = orelse {
                    let inner = region_any_generation_state(e, &sub, generation_dests);
                    sub = join_loan_generation_states(sub, &inner);
                }
                if let Some(fb) = finalbody {
                    let inner = region_any_generation_state(fb, &sub, generation_dests);
                    sub = join_loan_generation_states(sub, &inner);
                }
                any = join_loan_generation_states(any, &sub);
            }
            transfer_loan_generation(&mut state, instr, generation_dests);
            any = join_loan_generation_states(any, &state);
        }
    }
    any
}

/// The owner roots carried by each `EstablishLoans` generation. A later
/// establishment for the same aggregate replaces the old marker; keeping the
/// marker as the key prevents historical owners from becoming one permanent
/// dependency of the variable slot.
pub(super) struct DropLoanGeneration {
    pub(super) roots: Vec<VarId>,
    /// `ref` values, reference-bearing aggregates, and tracked pointers can
    /// leave a handle in an SSA register (a pointer argument is read inside
    /// its callee) and therefore participate in transient register
    /// provenance.
    pub(super) propagate_through_registers: bool,
}

/// Collected deep through `try` regions: region interiors get their own drop
/// elaboration, so their generations participate in owner retention too.
/// Markers are function-wide unique registers, so the extra entries are inert
/// for any walk that never activates them.
pub(super) fn drop_loan_generations(f: &MirFunction) -> BTreeMap<u32, DropLoanGeneration> {
    let mut roots_by_generation: BTreeMap<u32, DropLoanGeneration> = BTreeMap::new();
    for_each_instr_deep(&f.blocks, &mut |instr| {
        if let MirInstr::EstablishLoans { loans, marker, .. } = instr {
            // Every generation propagates, pointer variables included: a
            // pointer register handed to a call reads its pointee inside the
            // callee, after the variable's own last use.
            let generation =
                roots_by_generation
                    .entry(marker.0)
                    .or_insert_with(|| DropLoanGeneration {
                        roots: Vec::new(),
                        propagate_through_registers: true,
                    });
            for loan in loans {
                if !generation.roots.contains(&loan.place.root) {
                    generation.roots.push(loan.place.root);
                }
            }
        }
    });
    roots_by_generation
}

pub(super) fn effective_drop_liveness(
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

pub(super) fn active_drop_loan_roots(
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

pub(super) fn transfer_drop_liveness(
    instrs: &[MirInstr],
    register_uses: &[RegisterUses],
    mut live: HashSet<VarId>,
    live_in: &[HashSet<VarId>],
    function_defs: &HashMap<VarId, usize>,
) -> HashSet<VarId> {
    for (instr, uses) in instrs.iter().zip(register_uses).rev() {
        transfer_instruction_drop_liveness(&mut live, instr, uses, function_defs);
        live.extend(escape_live_in(instr, live_in));
    }
    live
}

/// What a `try`'s `break`/`continue` escape edges may still observe: the
/// live-in sets of their targets, which are live entering the `try` unless a
/// region rebinds them first (the entry-edge deaths then drop the old value).
pub(super) fn escape_live_in(instr: &MirInstr, live_in: &[HashSet<VarId>]) -> HashSet<VarId> {
    let mut targets = HashSet::new();
    try_escape_targets(instr, &mut targets);
    targets
        .into_iter()
        .filter_map(|target| live_in.get(target))
        .flatten()
        .copied()
        .collect()
}

/// The variables *used* anywhere in a region's blocks (recursively, through nested
/// `try`s — `var_uses` already descends into a `Try` instruction).
pub(super) fn region_uses(blocks: &[MirBlock]) -> HashSet<VarId> {
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

/// Collect the variables *local* to a `try`'s regions (every function-wide
/// def site — `DefVar` or `DeclareVar` — inside them, recursing through
/// nested `try`s) and the `^`-moved variables. Used to exclude a `try`'s own
/// region-locals (handled by `Try.cleanup`) and moved-out values from the
/// escape-edge cleanup; an outer variable declared or assigned outside a
/// region is *not* excluded, so the escape edge drops it exactly when it is
/// dead at the target.
pub(super) fn try_region_defs(
    try_instr: &MirInstr,
    function_defs: &HashMap<VarId, usize>,
    defs: &mut HashSet<VarId>,
    moved: &mut HashSet<VarId>,
) {
    let mut counts: HashMap<VarId, usize> = HashMap::new();
    let mut order: Vec<VarId> = Vec::new();
    collect_try_defs(try_instr, &mut counts, &mut order, moved);
    defs.extend(
        order
            .into_iter()
            .filter(|v| counts.get(v) == function_defs.get(v)),
    );
}

/// Collect every `EscapeJump` target block (a function-level block id) inside a
/// `try`'s regions, recursing through nested `try`s — the blocks whose live-in
/// sets bound what the escape edges may still observe.
pub(super) fn try_escape_targets(try_instr: &MirInstr, targets: &mut HashSet<usize>) {
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
                    try_escape_targets(instr, targets);
                }
                if let MirTerm::EscapeJump { target, .. } = &b.term {
                    targets.insert(*target);
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
pub(super) fn fill_escape_cleanups(
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
                for instr in &mut b.instrs {
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

/// Remove `ended` from every `EscapeJump.cleanup` under `blocks`, deep
/// through nested `try`s.
pub(super) fn strip_escape_cleanups(blocks: &mut [MirBlock], ended: &[VarId]) {
    if ended.is_empty() {
        return;
    }
    for block in blocks {
        for instr in &mut block.instrs {
            if let MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } = instr
            {
                strip_escape_cleanups(body, ended);
                if let Some((_, h)) = handler {
                    strip_escape_cleanups(h, ended);
                }
                if let Some(e) = orelse {
                    strip_escape_cleanups(e, ended);
                }
                if let Some(fb) = finalbody {
                    strip_escape_cleanups(fb, ended);
                }
            }
        }
        if let MirTerm::EscapeJump { cleanup, .. } = &mut block.term {
            cleanup.retain(|v| !ended.contains(v));
        }
    }
}

/// The live-out set of block `b`: the union of its successors' live-in sets.
pub(super) fn block_live_out(
    f: &MirFunction,
    b: usize,
    live_in: &[HashSet<VarId>],
) -> HashSet<VarId> {
    let mut out = HashSet::new();
    for s in terminator_targets(&f.blocks[b].term) {
        out.extend(&live_in[s]);
    }
    out
}

/// Number of predecessors of each block (from terminator successors).
pub(super) fn predecessor_counts(f: &MirFunction) -> Vec<usize> {
    let mut counts = vec![0usize; f.blocks.len()];
    for b in 0..f.blocks.len() {
        for s in terminator_targets(&f.blocks[b].term) {
            counts[s] += 1;
        }
    }
    counts
}

/// Append `DropVar`s for the given variables in reverse declaration order.
pub(super) fn append_drops(instrs: &mut Vec<MirInstr>, mut vars: Vec<VarId>) {
    vars.sort_unstable_by(|a, b| b.cmp(a));
    for v in vars {
        instrs.push(MirInstr::DropVar { var: v });
    }
}

/// Prepend `DropVar`s (reverse declaration order) to the front of a block.
pub(super) fn prepend_drops(instrs: &mut Vec<MirInstr>, mut vars: Vec<VarId>) {
    vars.sort_unstable_by(|a, b| b.cmp(a));
    for (i, v) in vars.into_iter().enumerate() {
        instrs.insert(i, MirInstr::DropVar { var: v });
    }
}

/// Redirect a terminator's `old` target to `new` (for critical-edge splitting).
pub(super) const fn rewire_target(term: &mut MirTerm, old: usize, new: usize) {
    match term {
        MirTerm::Jump(t) => {
            if *t == old {
                *t = new;
            }
        }
        MirTerm::Branch { then_b, else_b, .. }
        | MirTerm::ComptimeBranch { then_b, else_b, .. }
        | MirTerm::ComptimeFor {
            body: then_b,
            exit: else_b,
            ..
        } => {
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
        | MirTerm::EscapeJump { .. }
        | MirTerm::Yield(_)
        | MirTerm::ElifCond(_)
        | MirTerm::Break { .. }
        | MirTerm::Continue { .. }
        | MirTerm::ComptimeYield(_)
        | MirTerm::ComptimeBreak
        | MirTerm::ComptimeContinue
        | MirTerm::Unreachable => {}
    }
}

/// Backward transfer over a block for liveness.
pub(super) fn transfer_liveness(instrs: &[MirInstr], mut live: HashSet<VarId>) -> HashSet<VarId> {
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

/// One block's instructions with the analysis facts their drop elaboration
/// reads: the register-loan uses of each, and the loan generations entering
/// the block.
struct BlockReplay<'a> {
    instrs: Vec<MirInstr>,
    register_uses: &'a [RegisterUses],
    /// The function's `deinit` parameters, whose fields
    /// [`refine_deinit_fields`] destroys one by one: the variable's own
    /// teardown stays after the instruction.
    deinit_params: &'a [bool],
    generation_entry: &'a LoanGenerationState,
    loan_roots: &'a BTreeMap<u32, DropLoanGeneration>,
    generation_dests: &'a BTreeMap<u32, Option<MirInteriorOrigin>>,
    function_defs: &'a HashMap<VarId, usize>,
}

/// What a `try`'s own elaboration reads at its position in a replayed block.
struct TryPoint<'a> {
    index: usize,
    /// The `try` as the block holds it.
    instr: &'a MirInstr,
    /// Effective (loan-aware) liveness entering the `try`.
    live_before: &'a HashSet<VarId>,
    /// Effective liveness leaving it.
    live_after: &'a HashSet<VarId>,
    /// Ordinary liveness leaving it.
    ordinary_after: &'a HashSet<VarId>,
    generation_before: &'a LoanGenerationState,
}

impl BlockReplay<'_> {
    /// The block's instructions with a `DropVar` after each one for every
    /// variable that dies there, in one backward walk from `live_out`.
    ///
    /// A variable dies at an instruction when it is live (loan-aware) before
    /// it, or the instruction touches it, or a loan generation keeping it
    /// alive retires there, and it is dead after it. `seed` adds what else an
    /// instruction keeps live (escape-edge and raise-edge observers), and
    /// `at_try` elaborates a `try`'s regions in place, returning the
    /// variables to drop just before it and those a raise leaving it holds.
    /// A `try`'s regions own the deaths of what it uses, so a `try` is no
    /// death point. `raise_held`, when given, collects every slot a raise
    /// out of the block still holds: the effective liveness entering each
    /// raising instruction, which the elaboration leaves held.
    fn elaborate(
        self,
        live_out: HashSet<VarId>,
        seed: impl Fn(&MirInstr, &mut Vec<VarId>),
        droppable: impl Fn(VarId) -> bool,
        mut at_try: impl FnMut(&TryPoint, &mut MirInstr) -> (Vec<VarId>, HashSet<VarId>),
        mut raise_held: Option<&mut DenseSet>,
    ) -> Vec<MirInstr> {
        // The generation state before each instruction, as an index into the
        // distinct states the block passes through: most instructions leave
        // it unchanged.
        let mut generations = vec![self.generation_entry.clone()];
        let mut at = Vec::with_capacity(self.instrs.len() + 1);
        at.push(0);
        let mut state = self.generation_entry.clone();
        for instr in &self.instrs {
            transfer_loan_generation(&mut state, instr, self.generation_dests);
            if generations.last() != Some(&state) {
                generations.push(state.clone());
            }
            at.push(generations.len() - 1);
        }

        let mut live: DenseSet = live_out.into_iter().collect();
        let mut entering = Vec::new();
        let mut reversed = Vec::with_capacity(self.instrs.len());
        // An in-place binding's deaths, held until its producer is reached so
        // the producer's deaths join them after the binding.
        let mut pending: Option<InPlaceDeaths> = None;
        for (i, instr) in self.instrs.into_iter().enumerate().rev() {
            let generation_before = &generations[at[i]];
            let generation_after = &generations[at[i + 1]];
            let extra_after = loan_extras(&live, generation_after, self.loan_roots);
            let try_after = matches!(instr, MirInstr::Try { .. }).then(|| {
                let ordinary: HashSet<VarId> = live.iter().collect();
                let mut effective = ordinary.clone();
                effective.extend(extra_after.iter());
                (ordinary, effective)
            });

            // The backward step, remembering which variables it made live.
            let def = var_def(&instr);
            let uses = drop_liveness_uses(&instr, self.function_defs);
            let removed_def = def.is_some_and(|d| live.remove(d));
            entering.clear();
            entering.extend(&uses);
            seed(&instr, &mut entering);
            // A pending teardown keeps its variable live until then.
            entering.extend(teardown(&instr));
            // What the instruction itself, or an edge leaving it, still reads.
            let observed = entering.len();
            let register_uses = &self.register_uses[i];
            entering.extend(&register_uses.retained);
            entering.extend(&register_uses.loaded);
            let mut added = Vec::new();
            for v in &entering {
                if live.insert(*v) && !(removed_def && def == Some(*v)) {
                    added.push(*v);
                }
            }
            let extra_before = loan_extras(&live, generation_before, self.loan_roots);
            let live_after = |v: VarId| {
                (live.contains(v) && !added.contains(&v))
                    || (removed_def && def == Some(v))
                    || extra_after.contains(v)
            };

            let mut cloned = None;
            let entry_dead = match try_after {
                Some((ordinary_after, live_after)) => {
                    let mut live_before: HashSet<VarId> = live.iter().collect();
                    live_before.extend(extra_before.iter());
                    let elaborated = cloned.insert(instr.clone());
                    let point = TryPoint {
                        index: i,
                        instr: &instr,
                        live_before: &live_before,
                        live_after: &live_after,
                        ordinary_after: &ordinary_after,
                        generation_before,
                    };
                    let (entry_dead, held) = at_try(&point, elaborated);
                    if let Some(raise_held) = raise_held.as_deref_mut() {
                        raise_held.extend(held);
                    }
                    entry_dead
                }
                None => Vec::new(),
            };

            let mut moved = vars_moved(&instr);
            moved.extend(teardown(&instr));
            let retired: Vec<VarId> = if at[i] == at[i + 1] {
                Vec::new()
            } else {
                let roots_after = active_drop_loan_roots(generation_after, self.loan_roots);
                active_drop_loan_roots(generation_before, self.loan_roots)
                    .difference(&roots_after)
                    .copied()
                    .collect()
            };
            let deaths = added
                .iter()
                .copied()
                .chain(extra_before.iter())
                .chain(uses)
                .chain(def)
                .chain(register_uses.retained.iter().copied())
                .chain(register_uses.loaded.iter().copied())
                .chain(retired.iter().copied());
            // A `try` is no death point. Its regions are elaborated with the
            // liveness after it as their fall-off seed, so each region path
            // that reaches the fall-off already ends what the `try` uses or
            // holds live for an escape target; the entry edge (`at_try`), the
            // cleanup lists, and the escape cleanups end the rest.
            let ends_nothing = matches!(instr, MirInstr::Try { .. });
            let mut dying: Vec<VarId> = Vec::new();
            for v in deaths.filter(|_| !ends_nothing) {
                if droppable(v) && !moved.contains(&v) && !live_after(v) && !dying.contains(&v) {
                    dying.push(v);
                }
            }
            // An owner whose last use is a scalar the instruction loads by
            // value dies at that load, before the instruction runs.
            let deinit = |v: VarId| self.deinit_params.get(v as usize).copied() == Some(true);
            let loaded: Vec<VarId> = register_uses
                .loaded
                .iter()
                .copied()
                .filter(|v| {
                    dying.contains(v)
                        && !deinit(*v)
                        && !entering[..observed].contains(v)
                        && def != Some(*v)
                        && !retired.contains(v)
                        && !extra_before.contains(*v)
                })
                .collect();
            dying.retain(|v| !loaded.contains(v));
            if let Some(raise_held) = raise_held.as_deref_mut()
                && instr.is_raise_point()
            {
                let mut held = live.clone();
                held.union(&extra_before);
                for v in vars_moved(&instr).into_iter().chain(loaded.iter().copied()) {
                    held.remove(v);
                }
                raise_held.union(&held);
            }
            if let Some(held) = pending.take() {
                // The slot's previous value dying at the producer must drop
                // before the binding overwrites it, so that death point stays.
                if held.produced_by(&instr) && !dying.contains(&held.var) {
                    held.splice(&mut reversed, std::mem::take(&mut dying));
                } else {
                    held.splice(&mut reversed, Vec::new());
                }
            }
            match &instr {
                MirInstr::DefVar {
                    var,
                    src,
                    in_place: true,
                    ..
                } => {
                    pending = Some(InPlaceDeaths {
                        mark: reversed.len(),
                        var: *var,
                        src: *src,
                        dying,
                    });
                }
                _ => push_drops_reversed(&mut reversed, dying),
            }
            reversed.push(cloned.unwrap_or(instr));
            reversed.extend(
                loaded
                    .into_iter()
                    .rev()
                    .map(|var| MirInstr::DropVar { var }),
            );
            push_drops_reversed(&mut reversed, entry_dead);
        }
        if let Some(held) = pending {
            held.splice(&mut reversed, Vec::new());
        }
        reversed.reverse();
        reversed
    }
}

/// The variables dying at an in-place `DefVar`, held in the backward walk
/// until its producer: Mojo initializes the binding as the call's result
/// storage, so the call's own deaths (argument temporaries, owners last used
/// by the call) come after the binding, and the binding's slot, dead at its
/// definition, drops ahead of them.
struct InPlaceDeaths {
    /// Where the binding's trailing drops belong in the reversed output.
    mark: usize,
    var: VarId,
    src: Reg,
    dying: Vec<VarId>,
}

impl InPlaceDeaths {
    fn produced_by(&self, instr: &MirInstr) -> bool {
        let mut results = Vec::new();
        mojito_mir::mir::verify::instruction_result_regs(instr, &mut results);
        results.contains(&self.src)
    }

    /// Splice the binding's deaths and its producer's into `reversed`: the
    /// binding's slot first, the rest in descending `VarId`.
    fn splice(self, reversed: &mut Vec<MirInstr>, producer: Vec<VarId>) {
        let own = self.dying.contains(&self.var);
        let mut rest: Vec<VarId> = self
            .dying
            .into_iter()
            .filter(|v| *v != self.var)
            .chain(producer)
            .collect();
        rest.sort_unstable();
        rest.dedup();
        let drops = rest
            .into_iter()
            .chain(own.then_some(self.var))
            .map(|var| MirInstr::DropVar { var });
        reversed.splice(self.mark..self.mark, drops);
    }
}

/// One instruction's step of the backward drop-liveness transfer: its def
/// dies, and its direct reads, its pending `ConsumeVar`, and the roots it
/// consumes through its operand registers become live. Counting the register
/// uses here is what keeps an owner live across an edge when the register is
/// loaded in one block and consumed in another, as the replay counts them
/// within a block.
fn transfer_instruction_drop_liveness(
    live: &mut HashSet<VarId>,
    instr: &MirInstr,
    register_uses: &RegisterUses,
    function_defs: &HashMap<VarId, usize>,
) {
    if let Some(d) = var_def(instr) {
        live.remove(&d);
    }
    live.extend(drop_liveness_uses(instr, function_defs));
    // A pending teardown keeps its variable live up to that point so no
    // earlier death splices a competing `DropVar`.
    live.extend(teardown(instr));
    live.extend(&register_uses.retained);
    live.extend(&register_uses.loaded);
}

/// The variable an instruction tears down: a `ConsumeVar`, or a `DropVar`
/// already in the input. Its slot is live up to it and dead after it, so
/// elaboration splices no second drop of it.
const fn teardown(instr: &MirInstr) -> Option<VarId> {
    match instr {
        MirInstr::ConsumeVar { var } | MirInstr::DropVar { var } => Some(*var),
        _ => None,
    }
}

/// The variables an instruction reads, for drop liveness: [`var_uses`], less
/// the locals of a `try`'s regions. A slot only a `try`'s regions define —
/// the handler's error binding included — is scoped to the `try`: its
/// region's elaboration ends it on every exit but a raise, and the body's
/// `Try.cleanup` on the raise edge, so nothing outside observes it. Counting it as read would make it live
/// with no def, held round a loop's back edge and up to the function entry.
fn drop_liveness_uses(instr: &MirInstr, function_defs: &HashMap<VarId, usize>) -> Vec<VarId> {
    let uses = var_uses(instr).into_iter().map(|(v, _)| v);
    match instr {
        MirInstr::Try { .. } => {
            let (mut locals, mut moved) = (HashSet::new(), HashSet::new());
            try_region_defs(instr, function_defs, &mut locals, &mut moved);
            uses.filter(|v| !locals.contains(v)).collect()
        }
        _ => uses.collect(),
    }
}

/// The variables `live` keeps alive through the loan generations of `state`
/// beyond itself: the owner roots of every generation a live reference (or a
/// root kept alive this way) holds, as [`effective_drop_liveness`] closes
/// them.
fn loan_extras(
    live: &DenseSet,
    state: &LoanGenerationState,
    roots_by_generation: &BTreeMap<u32, DropLoanGeneration>,
) -> DenseSet {
    let mut extras = DenseSet::default();
    loop {
        let mut changed = false;
        for (reference, generations) in &state.active {
            if !live.contains(*reference) && !extras.contains(*reference) {
                continue;
            }
            for generation in generations {
                let Some(generation) = roots_by_generation.get(generation) else {
                    continue;
                };
                for root in &generation.roots {
                    if !live.contains(*root) {
                        changed |= extras.insert(*root);
                    }
                }
            }
        }
        if !changed {
            return extras;
        }
    }
}

/// [`append_drops`] for a block being built back to front.
fn push_drops_reversed(reversed: &mut Vec<MirInstr>, mut vars: Vec<VarId>) {
    vars.sort_unstable();
    reversed.extend(vars.into_iter().map(|var| MirInstr::DropVar { var }));
}

/// Seed a `try`'s raise-edge cleanup with the outer variables a raise in
/// its body must destroy: the per-instruction region drops place each
/// death on the *normal* path, so an outer value the body uses or rebinds
/// would leak when a raise lands before its in-region death. A candidate is
/// a non-local the body defines or any variable live entering the `try`
/// that the body uses, and it is seeded when nothing can observe it once
/// the body is left — dead on the normal continuation, unused by the
/// handler, `else`, `finally`, and the enclosing raise and exit observers
/// in `observed`, dead at every escape target, and not consumed in place by
/// the body (a `UseVar` move empties its slot, which a cleanup skips).
/// [`elaborate_try_interior`] keeps this seed beside the body-locals and
/// cuts both to what a raise holds.
fn seed_raise_cleanup(
    cloned: &mut MirInstr,
    point: &TryPoint,
    observed: &HashSet<VarId>,
    function_defs: &HashMap<VarId, usize>,
    effective_live_in: &[HashSet<VarId>],
    droppable: impl Fn(VarId) -> bool,
) {
    let MirInstr::Try {
        body,
        handler,
        orelse,
        finalbody,
        cleanup,
    } = cloned
    else {
        return;
    };
    let mut counts = HashMap::new();
    let (mut order, mut moved) = (Vec::new(), HashSet::new());
    collect_region_defs(body, &mut counts, &mut order, &mut moved);
    let mut survivors = observed.clone();
    for region in handler
        .iter()
        .map(|(_, h)| h)
        .chain(orelse.iter())
        .chain(finalbody.iter())
    {
        survivors.extend(region_uses(region));
    }
    let mut escape_targets = HashSet::new();
    try_escape_targets(point.instr, &mut escape_targets);
    let consumed = consumed_in_place(body);
    let used = region_uses(body);
    let mut outer: Vec<VarId> = point
        .live_before
        .iter()
        .copied()
        .filter(|v| used.contains(v) && !counts.contains_key(v))
        .collect();
    outer.sort_unstable();
    *cleanup = order
        .into_iter()
        .filter(|v| counts.get(v) != function_defs.get(v))
        .chain(outer)
        .filter(|v| {
            droppable(*v)
                && !consumed.contains(v)
                && !point.live_after.contains(v)
                && !survivors.contains(v)
                && !escape_targets.iter().any(|t| {
                    effective_live_in
                        .get(*t)
                        .is_some_and(|live| live.contains(v))
                })
        })
        .collect();
}

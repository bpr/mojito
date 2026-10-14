//! Flattened MIR data model shared by lowering, analysis, and execution.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

/// Whether the callee is responsible for running a parameter's `__deinit__` when
/// it reaches its last use still initialized — i.e. a plain consuming `var`
/// parameter (the caller transferred ownership; the callee must destroy it).
///
/// A `deinit` parameter is deliberately excluded: it is the destructor/
/// move-source convention (`__deinit__(deinit self)`, `__moveinit__(out self,
/// deinit other)`). Such a value is *consumed* — the function body transfers
/// its resources (move-only fields with `^`, copyable fields by copy) — so
/// auto-running its whole-value `__deinit__` at function end would destroy a value
/// whose resources already moved elsewhere, double-freeing it.
#[allow(
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    reason = "TODO: take Option<&T>; TODO: take by value and update the call sites"
)]
pub(super) const fn is_owned(c: &Option<ArgConvention>) -> bool {
    matches!(c, Some(ArgConvention::Var))
}

/// Whether an argument convention is `deinit` — the destructor/move-source
/// convention (`__deinit__(deinit self)`, `__moveinit__(out self, deinit other)`).
/// The callee is responsible for tearing the value down, but as a *consume*:
/// its residual fields are destroyed while its whole-value `__deinit__` is skipped
/// (its resources moved into the receiver), so drop elaboration emits
/// `ConsumeVar` rather than `DropVar` for such a parameter.
#[allow(
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    reason = "TODO: take Option<&T>; TODO: take by value and update the call sites"
)]
pub(super) const fn is_deinit(c: &Option<ArgConvention>) -> bool {
    matches!(c, Some(ArgConvention::Deinit))
}

/// Drop every block unreachable from the entry and renumber the targets
/// that survive.
///
/// The function level and each region are pruned alike, a region from
/// its own entry. A block a region's `EscapeJump` names is reachable through
/// the block holding the region. Registers, slots, and spans are untouched.
pub fn prune_unreachable_blocks(function: &mut MirFunction) {
    let kept = prune_block_list(&mut function.blocks);
    renumber_escapes(&mut function.blocks, &kept);
}

/// Every region an instruction holds, each a block list with local block
/// ids.
///
/// The order is fixed: an `if`'s then region, each elif's condition and
/// then regions, and its else region; a loop's body; a `comptime if`'s arms
/// and its else region; a `comptime for`'s body and else region; a `try`'s
/// body, handler, else, and finally parts.
pub fn instruction_regions(instruction: &MirInstr) -> Vec<&Vec<MirBlock>> {
    match instruction {
        MirInstr::If {
            then,
            elifs,
            orelse,
            ..
        } => std::iter::once(then)
            .chain(elifs.iter().flat_map(|elif| [&elif.cond, &elif.then]))
            .chain(std::iter::once(orelse))
            .collect(),
        MirInstr::Loop { body, .. } => vec![body],
        MirInstr::ComptimeIf { arms, orelse, .. } => {
            arms.iter().chain(std::iter::once(orelse)).collect()
        }
        MirInstr::ComptimeFor { body, orelse, .. } => vec![body, orelse],
        MirInstr::Try {
            body,
            handler,
            orelse,
            finalbody,
            ..
        } => std::iter::once(body)
            .chain(handler.iter().map(|(_, blocks)| blocks))
            .chain(orelse.iter())
            .chain(finalbody.iter())
            .collect(),
        _ => Vec::new(),
    }
}

/// [`instruction_regions`], mutably.
pub fn instruction_regions_mut(instruction: &mut MirInstr) -> Vec<&mut Vec<MirBlock>> {
    match instruction {
        MirInstr::If {
            then,
            elifs,
            orelse,
            ..
        } => std::iter::once(then)
            .chain(
                elifs
                    .iter_mut()
                    .flat_map(|elif| [&mut elif.cond, &mut elif.then]),
            )
            .chain(std::iter::once(orelse))
            .collect(),
        MirInstr::Loop { body, .. } => vec![body],
        MirInstr::ComptimeIf { arms, orelse, .. } => {
            arms.iter_mut().chain(std::iter::once(orelse)).collect()
        }
        MirInstr::ComptimeFor { body, orelse, .. } => vec![body, orelse],
        MirInstr::Try {
            body,
            handler,
            orelse,
            finalbody,
            ..
        } => std::iter::once(body)
            .chain(handler.iter_mut().map(|(_, blocks)| blocks))
            .chain(orelse.iter_mut())
            .chain(finalbody.iter_mut())
            .collect(),
        _ => Vec::new(),
    }
}

/// Every register a terminator reads, mutably.
pub fn terminator_regs_mut(term: &mut MirTerm) -> Vec<&mut Reg> {
    match term {
        MirTerm::Branch { cond, .. } | MirTerm::ElifCond(cond) => vec![cond],
        MirTerm::Return(Some(reg))
        | MirTerm::ReturnWithCleanup {
            value: Some(reg), ..
        } => vec![reg],
        MirTerm::Yield(values) | MirTerm::ComptimeYield(values) => values.iter_mut().collect(),
        MirTerm::Jump(_)
        | MirTerm::ComptimeBranch { .. }
        | MirTerm::ComptimeFor { .. }
        | MirTerm::Return(None)
        | MirTerm::ReturnWithCleanup { value: None, .. }
        | MirTerm::TryYield
        | MirTerm::EscapeJump { .. }
        | MirTerm::Break { .. }
        | MirTerm::Continue { .. }
        | MirTerm::ComptimeBreak
        | MirTerm::ComptimeContinue
        | MirTerm::Unreachable => Vec::new(),
    }
}

/// Every block of the same list a terminator names: a `try` region's
/// escape targets are the enclosing function's and are not among them.
pub fn terminator_targets(term: &MirTerm) -> Vec<MirBlockId> {
    match term {
        MirTerm::Jump(to) => vec![*to],
        MirTerm::Branch { then_b, else_b, .. }
        | MirTerm::ComptimeBranch { then_b, else_b, .. }
        | MirTerm::ComptimeFor {
            body: then_b,
            exit: else_b,
            ..
        } => vec![*then_b, *else_b],
        MirTerm::Return(_)
        | MirTerm::ReturnWithCleanup { .. }
        | MirTerm::TryYield
        | MirTerm::EscapeJump { .. }
        | MirTerm::Yield(_)
        | MirTerm::ElifCond(_)
        | MirTerm::Break { .. }
        | MirTerm::Continue { .. }
        | MirTerm::ComptimeYield(_)
        | MirTerm::ComptimeBreak
        | MirTerm::ComptimeContinue
        | MirTerm::Unreachable => Vec::new(),
    }
}

/// [`terminator_targets`], mutably.
pub fn terminator_targets_mut(term: &mut MirTerm) -> Vec<&mut MirBlockId> {
    match term {
        MirTerm::Jump(to) => vec![to],
        MirTerm::Branch { then_b, else_b, .. }
        | MirTerm::ComptimeBranch { then_b, else_b, .. }
        | MirTerm::ComptimeFor {
            body: then_b,
            exit: else_b,
            ..
        } => vec![then_b, else_b],
        MirTerm::Return(_)
        | MirTerm::ReturnWithCleanup { .. }
        | MirTerm::TryYield
        | MirTerm::EscapeJump { .. }
        | MirTerm::Yield(_)
        | MirTerm::ElifCond(_)
        | MirTerm::Break { .. }
        | MirTerm::Continue { .. }
        | MirTerm::ComptimeYield(_)
        | MirTerm::ComptimeBreak
        | MirTerm::ComptimeContinue
        | MirTerm::Unreachable => Vec::new(),
    }
}

/// Every compile-time argument slot an instruction carries, mutably.
///
/// They are a call's bracket arguments, a subscript call's, and a
/// constructed element, and none of a `try` region's blocks, which the
/// caller walks itself.
pub fn instruction_param_args_mut(instruction: &mut MirInstr) -> Vec<&mut MirParamArg> {
    match instruction {
        MirInstr::ConstructTypeParam { element, .. } => element.iter_mut().collect(),
        MirInstr::Call { param_arg_regs, .. }
        | MirInstr::CallIndirect { param_arg_regs, .. }
        | MirInstr::MethodCall { param_arg_regs, .. } => param_arg_regs.iter_mut().collect(),
        MirInstr::Index { call, .. }
        | MirInstr::Slice { call, .. }
        | MirInstr::MultiIndex { call, .. } => call
            .iter_mut()
            .flat_map(|call| call.param_arg_regs.iter_mut())
            .collect(),
        MirInstr::MultiSet { call, .. } => call.param_arg_regs.iter_mut().collect(),
        _ => Vec::new(),
    }
}

/// Every register an instruction defines or reads, mutably.
///
/// The registers of its places and subscript arguments are included, and
/// none of a region's blocks, which the caller walks itself.
/// Renumbering a copied block rewrites them all.
pub fn instruction_regs_mut(instruction: &mut MirInstr) -> Vec<&mut Reg> {
    fn subscript_arg<'a>(arg: &'a mut MirSubscriptArg, out: &mut Vec<&'a mut Reg>) {
        match arg {
            MirSubscriptArg::Index(reg) => out.push(reg),
            MirSubscriptArg::Slice {
                lower, upper, step, ..
            } => out.extend([lower, upper, step].into_iter().flatten()),
        }
    }
    fn param_args<'a>(args: &'a mut [MirParamArg], out: &mut Vec<&'a mut Reg>) {
        out.extend(args.iter_mut().filter_map(|arg| arg.value.as_mut()));
    }
    fn subscript_call<'a>(call: &'a mut MirSubscriptCall, out: &mut Vec<&'a mut Reg>) {
        param_args(&mut call.param_arg_regs, out);
    }
    let mut out = Vec::new();
    match instruction {
        MirInstr::EstablishLoans { loans, marker, .. } => {
            places_regs_mut(loans.iter_mut().map(|loan| &mut loan.place), &mut out);
            out.push(marker);
        }
        MirInstr::InvalidateInteriors { marker, .. } => out.push(marker),
        MirInstr::MakeRef { dest, place }
        | MirInstr::MovePlace { dest, place }
        | MirInstr::LoadPlace { dest, place } => {
            out.push(dest);
            place_regs_mut(place, &mut out);
        }
        MirInstr::ReadRef { dest, reference } => out.extend([dest, reference]),
        MirInstr::CopyValue { dest, value } | MirInstr::Rebind { dest, value } => {
            out.extend([dest, value]);
        }
        MirInstr::WriteRef { reference, value } => out.extend([reference, value]),
        MirInstr::MakeClosure { dest, captures, .. } => {
            out.push(dest);
            places_regs_mut(
                captures.iter_mut().map(|capture| &mut capture.place),
                &mut out,
            );
        }
        MirInstr::KeepAlive { .. }
        | MirInstr::DeclareVar { .. }
        | MirInstr::DropVar { .. }
        | MirInstr::ConsumeVar { .. }
        | MirInstr::GetIter { .. }
        | MirInstr::Unsupported(_)
        | MirInstr::Try { .. }
        | MirInstr::Loop { .. }
        | MirInstr::ComptimeFor { .. } => {}
        MirInstr::If { results, cond, .. } => out.extend(results.iter_mut().chain([cond])),
        MirInstr::ComptimeIf { results, .. } => out.extend(results.iter_mut()),
        MirInstr::ConstructTypeParam {
            dest,
            kwargs,
            kwarg_places,
            element,
            ..
        } => {
            out.push(dest);
            keyword_regs_mut(kwargs, kwarg_places, &mut out);
            param_args(element.as_mut_slice(), &mut out);
        }
        MirInstr::Const { dest, .. }
        | MirInstr::ConstructType { dest, .. }
        | MirInstr::SizeOf { dest, .. }
        | MirInstr::ParamListAddress { dest, .. }
        | MirInstr::TypeName { dest, .. }
        | MirInstr::HasNext { dest, .. }
        | MirInstr::Next { dest, .. } => out.push(dest),
        MirInstr::MaterializeLiteral { dest, value, .. } => out.extend([dest, value]),
        MirInstr::StoreToMem { dest, src } => out.extend([dest, src]),
        MirInstr::UseVar { dest, .. } => out.push(dest),
        MirInstr::DefVar { src, .. } => out.push(src),
        MirInstr::UnOp { dest, a, .. } => out.extend([dest, a]),
        MirInstr::BinOp { dest, a, b, .. } => out.extend([dest, a, b]),
        MirInstr::Call {
            dest,
            args,
            kwargs,
            arg_places,
            kwarg_places,
            param_arg_regs,
            ..
        } => {
            out.push(dest);
            out.extend(args.iter_mut());
            places_regs_mut(arg_places.iter_mut().flatten(), &mut out);
            keyword_regs_mut(kwargs, kwarg_places, &mut out);
            param_args(param_arg_regs, &mut out);
        }
        MirInstr::CallIndirect {
            dest,
            callee,
            args,
            kwargs,
            callee_place,
            arg_places,
            kwarg_places,
            param_arg_regs,
            ..
        } => {
            out.extend([dest, callee]);
            out.extend(args.iter_mut());
            places_regs_mut(callee_place.as_deref_mut(), &mut out);
            places_regs_mut(arg_places.iter_mut().flatten(), &mut out);
            keyword_regs_mut(kwargs, kwarg_places, &mut out);
            param_args(param_arg_regs, &mut out);
        }
        MirInstr::MethodCall {
            dest,
            recv,
            args,
            kwargs,
            recv_place,
            arg_places,
            kwarg_places,
            param_arg_regs,
            ..
        } => {
            out.extend([dest, recv]);
            out.extend(args.iter_mut());
            places_regs_mut(recv_place.as_deref_mut(), &mut out);
            places_regs_mut(arg_places.iter_mut().flatten(), &mut out);
            keyword_regs_mut(kwargs, kwarg_places, &mut out);
            param_args(param_arg_regs, &mut out);
        }
        MirInstr::PointerStorageTake {
            dest,
            pointer,
            index,
            ..
        }
        | MirInstr::PointerStorageDestroy {
            dest,
            pointer,
            index,
            ..
        } => out.extend([dest, pointer, index]),
        MirInstr::UninitStorage { dest, init } => {
            out.push(dest);
            out.extend(init.iter_mut());
        }
        MirInstr::UninitStorageTake { dest, storage, .. }
        | MirInstr::UninitStorageDestroy { dest, storage, .. } => out.extend([dest, storage]),
        MirInstr::GetField { dest, base, .. } => out.extend([dest, base]),
        MirInstr::Index {
            dest,
            base,
            index,
            base_place,
            index_place,
            call,
            ..
        } => {
            out.extend([dest, base, index]);
            places_regs_mut(base_place.as_deref_mut(), &mut out);
            places_regs_mut(index_place.as_deref_mut(), &mut out);
            if let Some(call) = call {
                subscript_call(call, &mut out);
            }
        }
        MirInstr::Slice {
            dest,
            object,
            lower,
            upper,
            step,
            object_place,
            arg_places,
            call,
            ..
        } => {
            out.extend([dest, object]);
            out.extend([lower, upper, step].into_iter().flatten());
            places_regs_mut(object_place.as_deref_mut(), &mut out);
            places_regs_mut(arg_places.iter_mut().flatten(), &mut out);
            if let Some(call) = call {
                subscript_call(call, &mut out);
            }
        }
        MirInstr::MultiIndex {
            dest,
            object,
            args,
            object_place,
            arg_places,
            kwargs,
            kwarg_places,
            call,
        } => {
            out.extend([dest, object]);
            for arg in args.iter_mut().chain(kwargs.iter_mut().map(|(_, arg)| arg)) {
                subscript_arg(arg, &mut out);
            }
            places_regs_mut(object_place.as_deref_mut(), &mut out);
            places_regs_mut(arg_places.iter_mut().flatten(), &mut out);
            places_regs_mut(kwarg_places.iter_mut().flatten(), &mut out);
            if let Some(call) = call {
                subscript_call(call, &mut out);
            }
        }
        MirInstr::MultiSet {
            receiver,
            receiver_place,
            args,
            arg_places,
            value,
            value_place,
            call,
            ..
        } => {
            out.extend([receiver, value]);
            for arg in args {
                subscript_arg(arg, &mut out);
            }
            places_regs_mut(receiver_place.as_deref_mut(), &mut out);
            places_regs_mut(arg_places.iter_mut().flatten(), &mut out);
            places_regs_mut(value_place.as_deref_mut(), &mut out);
            subscript_call(call, &mut out);
        }
        MirInstr::Store { place: target, src } => {
            place_regs_mut(target, &mut out);
            out.push(src);
        }
        MirInstr::StoreRef {
            place: target,
            reference,
        } => {
            place_regs_mut(target, &mut out);
            out.push(reference);
        }
        MirInstr::MakeTuple { dest, elems, .. } | MirInstr::MakeSimd { dest, elems, .. } => {
            out.push(dest);
            out.extend(elems.iter_mut());
        }
        MirInstr::MakeVariant { dest, value, .. } => out.extend([dest, value]),
        MirInstr::VariantIs { dest, variant, .. }
        | MirInstr::VariantGet { dest, variant, .. }
        | MirInstr::VariantTake { dest, variant, .. } => out.extend([dest, variant]),
        MirInstr::VariantSet {
            dest,
            place: target,
            value,
            ..
        }
        | MirInstr::VariantReplace {
            dest,
            place: target,
            value,
            ..
        } => {
            out.extend([dest, value]);
            place_regs_mut(target, &mut out);
        }
        MirInstr::VariantSetInitWith {
            dest,
            place: target,
            factory,
            ..
        } => {
            out.extend([dest, factory]);
            place_regs_mut(target, &mut out);
        }
        MirInstr::VariantDeinitWith {
            dest,
            variant,
            handler,
            ..
        } => out.extend([dest, variant, handler]),
        MirInstr::SimdCast { dest, value, .. } | MirInstr::SimdBitcast { dest, value, .. } => {
            out.extend([dest, value]);
        }
        MirInstr::SimdShuffle {
            dest, value, other, ..
        } => {
            out.extend([dest, value]);
            out.extend(other.iter_mut());
        }
        MirInstr::Raise { src } => out.push(src),
        MirInstr::Drop { reg } => out.push(reg),
        MirInstr::ConsumePlace {
            place: target,
            marker,
        } => {
            place_regs_mut(target, &mut out);
            out.push(marker);
        }
        MirInstr::DropPlace { place: target }
        | MirInstr::MarkInitialized { place: target }
        | MirInstr::MarkDestroyed { place: target } => place_regs_mut(target, &mut out),
        MirInstr::TryNext { dest, yielded, .. } => out.extend([dest, yielded]),
    }
    out
}

/// Every slot an instruction names: its place roots and `through` slots,
/// its capture-access roots, loan and interior roots, and the slot operands
/// of the variable and iterator instructions.
///
/// None of a region op's blocks, nor a `try`'s handler binding or cleanup,
/// are included; the caller walks those itself. [`for_each_instruction_slot_mut`]
/// visits the same slots in the same order.
pub fn instruction_slots(instruction: &MirInstr) -> Vec<VarId> {
    let mut out = Vec::new();
    for place in super::verify::instruction_places(instruction) {
        out.push(place.root);
        out.extend(place.through);
    }
    out.extend(
        capture_accesses(instruction)
            .iter()
            .map(|access| access.root),
    );
    match instruction {
        MirInstr::EstablishLoans {
            reference,
            loans,
            dest_interior,
            ..
        } => {
            out.push(*reference);
            out.extend(
                loans
                    .iter()
                    .filter_map(|loan| loan.interior.as_ref())
                    .chain(dest_interior.as_ref())
                    .map(|interior| interior.root),
            );
        }
        MirInstr::InvalidateInteriors { base, except, .. } => {
            out.push(base.root);
            out.extend(*except);
        }
        MirInstr::KeepAlive { var }
        | MirInstr::UseVar { var, .. }
        | MirInstr::DefVar { var, .. }
        | MirInstr::DeclareVar { var }
        | MirInstr::DropVar { var }
        | MirInstr::ConsumeVar { var } => out.push(*var),
        MirInstr::GetIter { source, dest, .. } => out.extend([*source, *dest]),
        MirInstr::HasNext { iter, .. }
        | MirInstr::Next { iter, .. }
        | MirInstr::TryNext { iter, .. } => out.push(*iter),
        _ => {}
    }
    out
}

/// Visit every slot [`instruction_slots`] lists, mutably and in the same
/// order. A visitor rather than a list: the places and the slot operands
/// borrow the instruction apart.
pub fn for_each_instruction_slot_mut(
    instruction: &mut MirInstr,
    visit: &mut impl FnMut(&mut VarId),
) {
    for place in super::verify::instruction_places_mut(instruction) {
        visit(&mut place.root);
        if let Some(through) = &mut place.through {
            visit(through);
        }
    }
    for access in capture_accesses_mut(instruction) {
        visit(&mut access.root);
    }
    match instruction {
        MirInstr::EstablishLoans {
            reference,
            loans,
            dest_interior,
            ..
        } => {
            visit(reference);
            for interior in loans
                .iter_mut()
                .filter_map(|loan| loan.interior.as_mut())
                .chain(dest_interior.as_mut())
            {
                visit(&mut interior.root);
            }
        }
        MirInstr::InvalidateInteriors { base, except, .. } => {
            visit(&mut base.root);
            if let Some(except) = except {
                visit(except);
            }
        }
        MirInstr::KeepAlive { var }
        | MirInstr::UseVar { var, .. }
        | MirInstr::DefVar { var, .. }
        | MirInstr::DeclareVar { var }
        | MirInstr::DropVar { var }
        | MirInstr::ConsumeVar { var } => visit(var),
        MirInstr::GetIter { source, dest, .. } => {
            visit(source);
            visit(dest);
        }
        MirInstr::HasNext { iter, .. }
        | MirInstr::Next { iter, .. }
        | MirInstr::TryNext { iter, .. } => visit(iter),
        _ => {}
    }
}

/// The capture accesses an instruction records: static ownership facts
/// whose roots name the same slots the body addresses.
fn capture_accesses(instruction: &MirInstr) -> &[MirCaptureAccess] {
    match instruction {
        MirInstr::Call {
            capture_accesses, ..
        }
        | MirInstr::CallIndirect {
            capture_accesses, ..
        }
        | MirInstr::MethodCall {
            capture_accesses, ..
        } => capture_accesses,
        MirInstr::Index {
            call: Some(call), ..
        }
        | MirInstr::Slice {
            call: Some(call), ..
        }
        | MirInstr::MultiIndex {
            call: Some(call), ..
        }
        | MirInstr::MultiSet { call, .. } => &call.capture_accesses,
        _ => &[],
    }
}

/// [`capture_accesses`], mutably.
fn capture_accesses_mut(instruction: &mut MirInstr) -> &mut [MirCaptureAccess] {
    match instruction {
        MirInstr::Call {
            capture_accesses, ..
        }
        | MirInstr::CallIndirect {
            capture_accesses, ..
        }
        | MirInstr::MethodCall {
            capture_accesses, ..
        } => capture_accesses,
        MirInstr::Index {
            call: Some(call), ..
        }
        | MirInstr::Slice {
            call: Some(call), ..
        }
        | MirInstr::MultiIndex {
            call: Some(call), ..
        }
        | MirInstr::MultiSet { call, .. } => &mut call.capture_accesses,
        _ => &mut [],
    }
}

/// The index registers of a place's projections.
fn place_regs_mut<'a>(place: &'a mut MirPlace, out: &mut Vec<&'a mut Reg>) {
    for projection in &mut place.proj {
        if let Proj::Index(reg) = projection {
            out.push(reg);
        }
    }
}

/// The index registers of every place.
fn places_regs_mut<'a>(
    places: impl IntoIterator<Item = &'a mut MirPlace>,
    out: &mut Vec<&'a mut Reg>,
) {
    for item in places {
        place_regs_mut(item, out);
    }
}

/// A call's keyword registers, then those of the places they read.
fn keyword_regs_mut<'a>(
    kwargs: &'a mut [(String, Reg)],
    kwarg_places: &'a mut [Option<MirPlace>],
    out: &mut Vec<&'a mut Reg>,
) {
    out.extend(kwargs.iter_mut().map(|(_, reg)| reg));
    places_regs_mut(kwarg_places.iter_mut().flatten(), out);
}

/// Prune `blocks` from block 0 and renumber their local targets; the
/// returned table maps each old index to its new one. Nested regions are
/// pruned the same way, and their escape targets are left for the caller,
/// since they name the enclosing function's blocks.
fn prune_block_list(blocks: &mut Vec<MirBlock>) -> Vec<Option<usize>> {
    let mut reachable = vec![false; blocks.len()];
    let mut pending = vec![0usize];
    while let Some(block) = pending.pop() {
        if block >= blocks.len() || std::mem::replace(&mut reachable[block], true) {
            continue;
        }
        pending.extend(block_successors(&blocks[block]));
    }
    let mut kept = vec![None; blocks.len()];
    let mut next = 0;
    for (index, reached) in reachable.iter().enumerate() {
        if *reached {
            kept[index] = Some(next);
            next += 1;
        }
    }
    let mut index = 0;
    blocks.retain(|_| {
        let keep = reachable[index];
        index += 1;
        keep
    });
    let target = |old: &mut MirBlockId| {
        *old = kept[*old].expect("a surviving block jumps only to surviving blocks");
    };
    for block in blocks.iter_mut() {
        terminator_targets_mut(&mut block.term)
            .into_iter()
            .for_each(target);
        for region in block.instrs.iter_mut().flat_map(instruction_regions_mut) {
            prune_block_list(region);
        }
    }
    kept
}

/// The blocks of the same list a block hands control to: its terminator's
/// targets, and the escape targets of every region it holds.
pub fn block_successors(block: &MirBlock) -> Vec<MirBlockId> {
    let mut successors = terminator_targets(&block.term);
    for region in block.instrs.iter().flat_map(instruction_regions) {
        collect_escape_targets(region, &mut successors);
    }
    successors
}

fn collect_escape_targets(blocks: &[MirBlock], targets: &mut Vec<MirBlockId>) {
    for block in blocks {
        if let MirTerm::EscapeJump { target, .. } = &block.term {
            targets.push(*target);
        }
        for region in block.instrs.iter().flat_map(instruction_regions) {
            collect_escape_targets(region, targets);
        }
    }
}

/// Renumber every escape target below `blocks` with the function-level table.
fn renumber_escapes(blocks: &mut [MirBlock], kept: &[Option<usize>]) {
    for block in blocks {
        for region in block.instrs.iter_mut().flat_map(instruction_regions_mut) {
            for inner in region.iter_mut() {
                if let MirTerm::EscapeJump { target, .. } = &mut inner.term {
                    *target = kept[*target].expect("an escape names a surviving block");
                }
            }
            renumber_escapes(region, kept);
        }
    }
}

/// Whether an argument convention is a written-back reference (`mut`/`ref`).
#[allow(
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    reason = "TODO: take Option<&T>; TODO: take by value and update the call sites"
)]
pub(super) const fn is_ref(c: &Option<ArgConvention>) -> bool {
    ArgConvention::is_reference(*c)
}
use mojito_hir::hir::VarId;
use std::collections::HashMap;

/// A virtual ("infinite") register — a fresh one per intermediate value (SSA-ish).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Reg(pub u32);

/// One source `[...]` argument after lowering.
///
/// `name` preserves keyword binding (`callback=increment`); `value` is absent
/// for an erased type argument. Semantic Origin/OriginSet arguments do not
/// enter this list.
///
/// Keeping binding identity separate from the optional runtime register lets
/// the VM normalize arguments to declaration order before applying defaults.
/// A bare `None` register cannot express the difference between an omitted
/// parameter and a supplied type argument.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MirParamArg {
    pub name: Option<String>,
    /// The argument's run-time register; absent for an erased origin
    /// argument, for a type argument, and for a closed value the call's
    /// `instantiated_args` carry.
    pub value: Option<Reg>,
    /// A value argument built from the enclosing declaration's value binders
    /// (`successor[n, 1 + n]()`, `below[Self.n, k]()`), as the expression
    /// over them that monomorphization evaluates under the caller instance's
    /// bindings. `value` still carries the runtime register the VM reads.
    pub expr: Option<mojito_types::param_expr::ParamExpr>,
}

/// Index of a basic block within a [`MirFunction`]'s `blocks`.
pub type MirBlockId = usize;

/// A source byte range `(start, end)` — re-exported from [`mojito_common::token`], the
/// canonical span type stamped by the parser onto every AST node.
pub use mojito_common::token::Span;

/// How a variable is used at a given site (set from `^` and param conventions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum UseMode {
    Copy,
    /// End the slot and move-initialize a new value from it.
    Move,
    BorrowShared,
    BorrowMut,
    /// End the slot and hand its storage to the register's consumer with no
    /// move initializer: an owned (`var`/`deinit`) parameter of the call that
    /// consumes the register, as Mojo's owned argument convention binds the
    /// caller's storage, or a conditional expression's result temporary,
    /// which its consumer takes as any other rvalue, or a discard (`_ = x^`),
    /// whose slot destroys the storage in place. Ownership treats it
    /// exactly as [`UseMode::Move`].
    Forward,
}

impl UseMode {
    /// Whether the use ends the slot: [`UseMode::Move`] or
    /// [`UseMode::Forward`].
    pub const fn moves(self) -> bool {
        matches!(self, Self::Move | Self::Forward)
    }
}

/// The name prefix of a forwarding slot.
///
/// A fresh temporary an owned parameter or owned receiver takes
/// (`take(M(copy=y))`, `take(M(7))`) is materialized in it and forwarded
/// once, at the call, so a raise in a later operand destroys it through the
/// slot. A copy construction is bound in place. When the copy's source dies
/// there, copy elision forwards the source's storage instead.
pub const FORWARDED_ARGUMENT_SLOT: &str = "$fwd_arg_r";

/// The name prefix of a read-temporary slot.
///
/// A loan-free owned temporary a read parameter, a read receiver, or a read
/// operator operand takes (`show(B(2))`, `B(2).show()`) is bound to it and read once by the call. When the
/// temporary is a copy whose source dies there, copy elision lends the
/// source's storage instead.
pub const READ_TEMPORARY_SLOT: &str = "$tmp_arg_r";

/// One already-evaluated argument to a multi-dimensional subscript.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum MirSubscriptArg {
    Index(Reg),
    Slice {
        kind: mojito_types::types::SliceKind,
        lower: Option<Reg>,
        upper: Option<Reg>,
        step: Option<Reg>,
    },
}

/// Checked non-nominal dispatch for an index or slice instruction.
///
/// A missing [`MirSubscriptCall`] is never an invitation for the VM to inspect
/// the runtime value and guess semantics: lowering records the exact
/// compiler/runtime storage family selected by the checked base type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MirIntrinsicSubscript {
    /// Compiler-private heterogeneous Tuple/RuntimePack storage, including a
    /// nominal Tuple result whose intrinsic producer uses `Value::Tuple` as its
    /// ABI (currently `Slice.indices`).
    TupleStorage,
    /// Compiler-private homogeneous `*args` storage.
    VariadicStorage,
    Simd,
    Pointer,
    ComptimeList,
}

/// A compile-time-known literal.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Const {
    Int(i64),
    Float(f64),
    IntLiteral(mojito_common::literal::IntLiteral),
    FloatLiteral(mojito_common::literal::FloatLiteral),
    Bool(bool),
    Str(String),
    Function(String),
    /// A `DType` value.
    Dtype(mojito_ast::ast::Dtype),
    None,
    /// A parameter expression read as a runtime value (`Ts.length`,
    /// `len(Ts)`, `Ts.contains[X]()`): upstream's `kgen.param.constant` with
    /// a symbolic attribute. A generator carries it and the elaborator folds
    /// it under the instance's bindings; concrete MIR has none.
    Param(mojito_types::param_expr::ParamExpr),
    /// A closed typed value read as a runtime value: upstream's
    /// `kgen.param.constant` over a constant attribute. The elaborator folds
    /// a binder read to it (`Self.key` of an `AHasher[key: U256]` instance,
    /// a `UInt` parameter), and `sccp` folds a literal materialized to `UInt`
    /// or a one-lane SIMD type, or a splat or per-lane construction of a SIMD type of any width, to it. It holds a `CtValue::UInt`, a
    /// `CtValue::Simd`, or a `CtValue::Struct` of such values and scalars.
    Value(mojito_types::ct::CtValue),
}

impl Const {
    /// Whether this is a scalar constant `sccp` tracks: a built-in scalar or
    /// literal, a `UInt` value, or a one-lane SIMD value.
    pub const fn is_folded_scalar(&self) -> bool {
        match self {
            Self::Int(_)
            | Self::Float(_)
            | Self::Bool(_)
            | Self::IntLiteral(_)
            | Self::FloatLiteral(_)
            | Self::Value(CtValue::UInt(_)) => true,
            Self::Value(CtValue::Simd { lanes, .. }) => lanes.len() == 1,
            _ => false,
        }
    }

    /// Whether this is a constant `sccp` tracks: a folded scalar or a SIMD
    /// value of any width.
    pub const fn is_folded(&self) -> bool {
        match self {
            Self::Value(CtValue::Simd { lanes, .. }) => !lanes.is_empty(),
            _ => self.is_folded_scalar(),
        }
    }
}

/// The callee of a `MirInstr::Call` — a function/struct-constructor/builtin name.
/// (Resolution to a concrete target happens in the backend's assembler.)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FuncRef(pub String);
impl FuncRef {
    pub fn named(name: &str) -> Self {
        Self(name.to_string())
    }
}

/// One step of a **place** projection: a field of a struct, or a subscript.
///
/// A place is a *writable location* — a root variable followed by projections
/// — as opposed to an rvalue (a computed register). This mirrors `rustc` MIR's
/// `Place`/`Projection` split, and is what a write / read-modify-write
/// targets.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Proj {
    Field(String),
    Index(Reg), // the subscript index, flattened to a register (evaluated once)
    /// A statically selected element of compiler-private heterogeneous Tuple
    /// storage. Unlike [`Proj::Index`], distinct constant indices are disjoint
    /// ownership paths, so moving element 0 does not move element 1.
    ConstIndex(usize),
    /// Payload of a checked `Variant` alternative.  The tag is static; runtime
    /// navigation traps if the active alternative differs.
    Variant(mojito_types::types::VariantIndex),
    /// Payload of compiler-private inline uninit storage (`__UninitStorage`).
    /// A final-step write initializes-or-overwrites without dropping the old
    /// payload; reads trap while the storage is uninitialized.
    UninitPayload,
    /// `rebind[Dest](place)`: the same storage addressed at `Dest`, the
    /// step's `projection_tys` entry, as upstream's reference `rebind`
    /// applies `kgen.rebind` to the pointer it returns. Storage, ownership,
    /// and execution read through it ([`MirPlace::storage`]); the elaborator
    /// judges and erases it, so concrete MIR never holds one.
    Rebind,
}

/// A writable location: a root variable plus a chain of projections
/// (`p.items[i].x` = root `p`, proj `[Field("items"), Index(i), Field("x")]`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MirPlace {
    pub root: VarId,
    /// Checked type of the root slot before projections. `None` is permitted only
    /// for compatibility HIR built without a `CheckedProgram`; production MIR
    /// verification rejects it.
    pub root_ty: Option<Ty>,
    pub proj: Vec<Proj>,
    /// Result type after each corresponding projection in `proj`.
    pub projection_tys: Vec<Ty>,
    /// Checked type of the designated storage after all projections.
    pub ty: Option<Ty>,
    /// The local reference through which this place is accessed. `None` means
    /// direct owner access. This is static metadata ignored by the VM.
    pub through: Option<VarId>,
}

/// A place's types are shared handles, so a place is its two lists and a
/// few words.
const _: () = assert!(std::mem::size_of::<MirPlace>() <= 80);

impl MirPlace {
    pub fn root(root: VarId, ty: Option<Ty>) -> Self {
        Self {
            root,
            root_ty: ty.clone(),
            proj: Vec::new(),
            projection_tys: Vec::new(),
            ty,
            through: None,
        }
    }

    pub fn project(&mut self, projection: Proj, ty: Ty) {
        self.proj.push(projection);
        self.projection_tys.push(ty.clone());
        self.ty = Some(ty);
    }

    pub const fn is_typed(&self) -> bool {
        self.root_ty.is_some() && self.ty.is_some() && self.proj.len() == self.projection_tys.len()
    }

    /// The storage this place designates: every [`Proj::Rebind`] step and
    /// its type removed, the terminal type kept. Borrowed when the place has
    /// no such step, as every concrete place.
    pub fn storage(&self) -> std::borrow::Cow<'_, Self> {
        if !self.proj.iter().any(|step| matches!(step, Proj::Rebind)) {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut storage = self.clone();
        let mut types = std::mem::take(&mut storage.projection_tys).into_iter();
        let (proj, projection_tys) = std::mem::take(&mut storage.proj)
            .into_iter()
            .zip(types.by_ref())
            .filter(|(step, _)| !matches!(step, Proj::Rebind))
            .unzip();
        storage.proj = proj;
        storage.projection_tys = projection_tys;
        storage.projection_tys.extend(types);
        std::borrow::Cow::Owned(storage)
    }

    /// The steps that select storage: every step but a [`Proj::Rebind`].
    pub fn steps(&self) -> std::iter::Filter<std::slice::Iter<'_, Proj>, fn(&&Proj) -> bool> {
        self.proj
            .iter()
            .filter(|step| !matches!(step, Proj::Rebind))
    }

    /// Whether this place designates its whole root, rebound or not.
    pub fn is_whole_root(&self) -> bool {
        self.proj.iter().all(|step| matches!(step, Proj::Rebind))
    }
}

/// Canonical, runtime-erased identity of an interior storage generation after
/// stable checker owners have been mapped to MIR slots.
///
/// `Interior` path segments are invalidation domains; ordinary field/index
/// segments retain field sensitivity.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct MirInteriorOrigin {
    pub root: VarId,
    pub path: Vec<mojito_types::origin::OriginSeg>,
}

/// One owner dependency carried by a reference or reference-bearing value.
///
/// `place` is the executable target; `interior` is the distinct analytical
/// generation identity when the target lives behind container-owned storage. A
/// `shared` loan (a `Pointer(to=place)` alias) never conflicts with another
/// shared loan of overlapping storage; against owner accesses and exclusive
/// loans it is an ordinary borrow. A `setup` loan holds a by-value call
/// operand's lvalue until its call's setup reads it: it keeps its owner live
/// and carries its interior generation, but excludes no access and no other
/// loan, and each read through it reads its owner, so a move between the
/// operand and setup is a use of a moved value.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MirLoan {
    pub place: MirPlace,
    pub mutable: bool,
    pub interior: Option<MirInteriorOrigin>,
    pub shared: bool,
    pub setup: bool,
}

/// Complete checker-selected contract for a nominal subscript invocation.
/// Intrinsic pointer/SIMD/private-storage operations carry no such payload.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MirSubscriptCall {
    pub target: String,
    pub raises: Option<Ty>,
    /// Checker-selected executable result type, including the instantiated
    /// origin and mutability of a reference result.
    pub result_ty: Ty,
    pub receiver_requires_place: bool,
    pub receiver_convention: Option<mojito_ast::ast::ArgConvention>,
    pub arguments: Vec<mojito_checked::checked::CheckedCallArgument>,
    pub capture_accesses: Vec<MirCaptureAccess>,
    pub reference_result: Option<mojito_types::origin::RefTy>,
    pub param_arg_regs: Vec<MirParamArg>,
    pub param_decls: Vec<mojito_types::types::ParamDecl>,
}

/// A single three-address instruction. Each value-producing instruction writes a
/// fresh `dest` register; control flow lives in the block's [`MirTerm`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum MirInstr {
    /// Establish one fresh generation containing every owner loan carried by a
    /// reference or aggregate binding. Grouping the loans makes rebinding reset
    /// the old generation atomically instead of merging historical loans for
    /// the same variable slot.
    EstablishLoans {
        reference: VarId,
        loans: Vec<MirLoan>,
        marker: Reg,
        /// Interior destination domain within `reference` that holds the
        /// loans (`None` ≡ the whole root). Rebinding a place that covers
        /// this domain releases the generation; sibling domains coexist.
        dest_interior: Option<MirInteriorOrigin>,
    },
    /// Invalidate established interior generations rooted below `base`.
    /// `include_base_generation` also replaces the exact named generation at
    /// `base`; `except` preserves the generation used to perform an ordinary
    /// mutation through an interior reference while still invalidating nested
    /// interiors.
    InvalidateInteriors {
        base: MirInteriorOrigin,
        except: Option<VarId>,
        include_base_generation: bool,
        marker: Reg,
    },
    /// Materialize a runtime reference handle to a verified place. If the root
    /// is already a reference parameter, its handle is forwarded and extended.
    MakeRef {
        dest: Reg,
        place: MirPlace,
    },
    ReadRef {
        dest: Reg,
        reference: Reg,
    },
    /// `rebind[Dest](value)` read as a value: `dest`, typed `Dest`, holds
    /// what `value` held, typed the operand's own type — upstream's
    /// `kgen.rebind`. Only a generator holds one; the elaborator asserts the
    /// two register types equal per instance and replaces `dest` by `value`.
    Rebind {
        dest: Reg,
        value: Reg,
    },
    /// Materialize an owned copy of a register value.  Reference-returning
    /// expressions use this after `ReadRef` in ordinary value contexts; an
    /// explicit `ref` binding retains the handle and therefore never emits it.
    /// Keeping the operation explicit prevents the VM from guessing whether a
    /// handle read is a borrow, a forwarding read, or a lifecycle copy.
    CopyValue {
        dest: Reg,
        value: Reg,
    },
    WriteRef {
        reference: Reg,
        value: Reg,
    },
    /// Build a non-escaping closure value from a lifted function and its explicit
    /// environment. The capture mode fixes whether declaration evaluation stores
    /// a reference handle, an owned snapshot, or a transferred value.
    MakeClosure {
        dest: Reg,
        function: String,
        captures: Vec<MirClosureCapture>,
    },
    /// Extend an owner's MIR live range through a closure invocation without
    /// performing a value-level copy. This is erased by execution.
    KeepAlive {
        var: VarId,
    },
    Const {
        dest: Reg,
        k: Const,
    },
    /// Construct the concrete type reified for a checked type parameter,
    /// through the bound's initializer the checker selected: `T()` passes no
    /// argument, `T(copy=x)` the `Copyable` initializer's one.
    ConstructTypeParam {
        dest: Reg,
        param: mojito_types::param_expr::ParamRef,
        /// The selected initializer's keyword arguments, each a read the
        /// initializer borrows.
        kwargs: Vec<(String, Reg)>,
        /// Like `Call::kwarg_places`, aligned with `kwargs`: the retained
        /// caller place each borrowed argument reads.
        kwarg_places: Vec<Option<MirPlace>>,
        /// `Ts[i]()`: `param` is a type pack and this argument the index of
        /// the element constructed, as `Call::param_arg_regs` carries one —
        /// `value` the register the erased VM reads, `expr` the expression
        /// the elaborator evaluates.
        element: Option<MirParamArg>,
    },
    /// Construct the type a compile-time parameter expression denotes,
    /// through the `Defaultable` initializer the checker proved of it: `FT()`
    /// and `types[i]()` over `reflect[T].field_types()`. Only a template
    /// holds one: the elaborator writes the closed type's default
    /// construction.
    ConstructType {
        dest: Reg,
        ty: Ty,
    },
    /// The byte size of one checker-resolved type: a layout query only a
    /// generator carries. The elaborator answers it under the compilation's
    /// native target, so elaborated MIR holds the constant and never the
    /// query.
    SizeOf {
        dest: Reg,
        ty: Ty,
    },
    /// The static address of a parameter list's elements, laid out once for
    /// the whole program as a constant array (upstream's `global_constant`
    /// over `#pop.variadic_to_array`): `dest` is a `Pointer[E,
    /// ImmStaticOrigin]` to the first element. A template holds the list as
    /// the `Const::Param` it denotes; the elaborator closes it to the
    /// `Const::Value` tuple of its elements.
    ParamListAddress {
        dest: Reg,
        values: Const,
    },
    /// Move `src` into compile-time memory and set `dest`, a reference
    /// typed by `dest`'s register, to it: upstream's `#lit.store_to_mem`,
    /// the `ref[ComptimeOrigin]` a parameter-domain value is borrowed
    /// through — immutable for a binding (`PMBValue`), mutable for a
    /// temporary (`PMRValue`). Only a lifted compile-time evaluation's body
    /// holds one, where it anchors a value the evaluation borrows (a local
    /// `comptime` binding, a folded receiver); compile-time memory is never
    /// destroyed, so a reference into it survives the evaluation's result.
    StoreToMem {
        dest: Reg,
        src: Reg,
    },
    /// The unqualified spelling of one checker-resolved type that names a
    /// compile-time parameter. Only a template holds one: the elaborator
    /// writes the name from the substituted type as a string constant.
    TypeName {
        dest: Reg,
        ty: Ty,
    },
    /// Cross the compile-time literal boundary selected by the checker.  The
    /// operand is an exact `IntLiteral`/`FloatLiteral`; `target` is a concrete
    /// scalar (or width-one scalar alias).  Backends must implement this
    /// conversion explicitly rather than infer it from a later store.
    MaterializeLiteral {
        dest: Reg,
        value: Reg,
        target: Ty,
    },
    /// `x`, `x^`, `borrow x`, … — a use of a variable, tagged with how (`mode`).
    UseVar {
        dest: Reg,
        var: VarId,
        mode: UseMode,
    },
    /// A **partial move** `p.a^`, or a constant-index move from compiler-private
    /// Tuple storage — transfer one independently owned sub-place out of a
    /// variable, reading its value into `dest`. The ownership analysis tracks
    /// this at place granularity (moving `p.a` leaves `p.b` usable, and moving
    /// Tuple element 0 leaves element 1 usable); at runtime the slot is left a
    /// tombstone so a later aggregate drop skips it. Whole-variable moves stay
    /// `UseVar { mode: Move | Forward }`; user-facing indexed transfers remain restricted
    /// by checking to copyable value reads.
    MovePlace {
        dest: Reg,
        place: MirPlace,
    },
    /// `var := <register>` — (re)define a variable slot from a register (lowered
    /// from a HIR `Bind`). The write paired with `UseVar`; Stage 6 reads it as a
    /// dataflow *def* (transitions the var to `Owned`). `binding_ty` is the
    /// checker-resolved destination type; source annotation syntax never enters
    /// MIR. `None` on reassignment keeps the slot's existing runtime type.
    /// `in_place` marks the binding as the result storage of the immediately
    /// preceding non-raising call-family instruction (Mojo's `out`-slot
    /// initialization): the binding is part of its producer, so the producer's
    /// deaths happen after it and the slot, dead at its definition, drops
    /// before the call's argument temporaries.
    DefVar {
        var: VarId,
        src: Reg,
        binding_ty: Option<Ty>,
        in_place: bool,
    },
    /// `var x: T` — the slot's declaration point, where its scope begins
    /// (Mojo gives a `var` its storage at the declaration, and the first
    /// assignment initializes it). It writes and reads nothing, is no
    /// liveness def or use, and is a no-op on both backends; region locality
    /// counts it as a def site, so a slot declared outside a `try` survives
    /// the body's exit however many of its stores lie inside.
    DeclareVar {
        var: VarId,
    },
    UnOp {
        op: PrefixOp,
        dest: Reg,
        a: Reg,
        /// The checked error type the operator method's call raises, as a
        /// call's `raises`; `None` for a built-in or non-raising operator.
        raises: Option<Ty>,
    },
    BinOp {
        op: InfixOp,
        dest: Reg,
        a: Reg,
        b: Reg,
        /// Checker-selected operator implementation when nominal overload
        /// resolution was required (notably a concrete Tuple membership
        /// overload). Backends must not reselect it by name/arity.
        resolved: Option<String>,
        /// The checked error type the operator call raises, as a call's
        /// `raises`; `None` for a built-in or non-raising operator.
        raises: Option<Ty>,
    },
    /// A free-function / constructor / builtin call. `args` are the flattened
    /// positional arguments; `kwargs` the keyword arguments (`name = value`). The
    /// backend matches them to the callee's parameter slots (filling defaults,
    /// collecting `*args`) via the phase-neutral call matcher.
    /// A free-function call. `arg_places[i]` is `Some` when positional argument
    /// `i` is a simple place (a variable or field chain, no dynamic index) and
    /// either checking selected a `mut`/`ref` parameter (the place is retained
    /// for handle passing/write-back) or the call's result is a borrowing view
    /// lending that argument (the place is retained as a shared read so the
    /// callee binds the caller's storage; the analysis classifies it by the
    /// callee's convention at that slot). Ordinary copied arguments and
    /// unsupported place shapes are `None`.
    Call {
        dest: Reg,
        func: FuncRef,
        /// Checker-selected error contract for this call, if it may raise.
        raises: Option<Ty>,
        args: Vec<Reg>,
        kwargs: Vec<(String, Reg)>,
        arg_places: Vec<Option<MirPlace>>,
        /// Retained caller places aligned with `kwargs`. Keyword binding is
        /// reordered by the call ABI, so the backend resolves these by the
        /// selected parameter name rather than by positional index.
        kwarg_places: Vec<Option<MirPlace>>,
        /// Concrete captured-owner effects performed transitively during this
        /// call. Static verification consumes these; execution erases them.
        capture_accesses: Vec<MirCaptureAccess>,
        /// Supplied compile-time arguments in source order. Each entry retains
        /// an optional keyword name and an optional value register; type
        /// arguments have no register. Origin arguments are semantically erased
        /// before this list. Consumers bind these entries to `ParamDecl` order
        /// before applying defaults.
        param_arg_regs: Vec<MirParamArg>,
        /// The spelled receiver of a static call on a generic struct
        /// (`Pair[Self.U].count()`), in the caller's binder scope. No runtime
        /// argument need carry the struct's parameters, so the elaborator
        /// binds them from this type; a resolved call keeps none.
        receiver: Option<Ty>,
        /// The compile-time arguments the checker solved for a call of a
        /// generic `def`, in declaration order and in the caller's binder
        /// scope. A type argument no runtime parameter or result spells
        /// (`bytes[Int]()`) is bound from here by the elaborator; a resolved
        /// call keeps none.
        instantiated_args: Vec<TyArg>,
        /// The position in `args` of a whole type pack spread into the
        /// callee's collector (`show(*args)`): the register holds the
        /// caller's collector, a `VariadicPack` over a pack still a
        /// parameter, and `arg_places` retains the collector's place when
        /// the pack is read. The elaborator replaces the argument with the
        /// bound pack's element places; concrete MIR carries none.
        spread: Option<usize>,
    },
    /// A call through a runtime function value. Callable parameters use this
    /// instruction instead of treating the parameter name as a global symbol.
    CallIndirect {
        dest: Reg,
        callee: Reg,
        /// Exact checker-selected nominal `__call__` target, or a
        /// signature-qualified abstract target for a `def(...)` value. The VM
        /// consults this only when `callee` is a nominal callable struct.
        resolved: Option<String>,
        /// Checker-selected error contract of the callable value.
        raises: Option<Ty>,
        args: Vec<Reg>,
        kwargs: Vec<(String, Reg)>,
        /// The callable value's caller place, when it has one. This is needed
        /// for a callable struct whose `__call__` receiver is `mut`/`ref`.
        callee_place: Option<Box<MirPlace>>,
        /// Like `Call::arg_places`: the retained caller place for each
        /// positional argument selected for a `mut`/`ref` parameter.
        arg_places: Vec<Option<MirPlace>>,
        /// Like `Call::kwarg_places`, aligned with `kwargs`.
        kwarg_places: Vec<Option<MirPlace>>,
        capture_accesses: Vec<MirCaptureAccess>,
        /// Compile-time arguments supplied while invoking a generic callable
        /// value, in the same source-order representation as `Call`.
        param_arg_regs: Vec<MirParamArg>,
        /// The generic callable contract selected by checking. Contract-side
        /// defaults govern omitted arguments even when the concrete function
        /// value declares different defaults.
        param_decls: Vec<ParamDecl>,
        /// Fully instantiated checker contract for this invocation, when
        /// explicit compile-time arguments resolve dependent parameter/result
        /// types. Generic callable storage may remain symbolic; executable
        /// operand verification uses this concrete view.
        instantiated_contract: Option<Ty>,
        /// Complete checker-resolved generic arguments in declaration order.
        /// This semantic-only substitution witness lets verification validate
        /// `instantiated_contract` without inspecting the instructions which
        /// materialized compile-time argument registers.
        instantiated_args: Vec<TyArg>,
        /// Like `Call::spread`: the position in `args` of a whole type pack
        /// spread into a generic nested `def`'s collector
        /// (`count(*args^)`). The elaborator expands it before it names the
        /// lifted body's instance; concrete MIR carries none.
        spread: Option<usize>,
    },
    /// A method call `recv.method(args)`. `recv_place` is `Some` when the receiver
    /// is a writable place (a variable / field-index chain), so a `mut self` method
    /// can write the updated receiver back. A temporary receiver (a call result)
    /// admits only read-only methods (the checker guarantees this); it is `None`
    /// unless the call result is a borrowing view, which is retained in a hidden
    /// `$view_recv_r` slot whose loans keep the borrowed source alive across the
    /// chained call.
    MethodCall {
        dest: Reg,
        recv: Reg,
        method: String,
        resolved: Option<String>,
        /// Checker-selected concrete or trait-requirement error contract.
        raises: Option<Ty>,
        /// Reference-return ABI retained independently from the destination
        /// value type. A value-returning method may itself produce a
        /// reference-valued element, so the verifier must not infer ABI from
        /// `reg_types[dest]`.
        reference_result: Option<Box<mojito_types::origin::RefTy>>,
        /// Checker-proven adaptation from a concrete implementation ABI to the
        /// abstract result promised at this call site.
        result_adapter: Option<mojito_checked::checked::CheckedResultAdapter>,
        args: Vec<Reg>,
        kwargs: Vec<(String, Reg)>,
        recv_place: Option<Box<MirPlace>>,
        /// Whether the call may write through `recv_place`: a `mut` or
        /// mutable `ref` receiver, or a consuming one. A borrowed `self`
        /// receiver only reads its place, so a live shared loan on that place
        /// (a reference result used as the receiver) does not conflict with
        /// the call. Unchecked lowering paths stay conservatively `true`.
        recv_writes: bool,
        /// Like `Call::arg_places`: `arg_places[i]` is `Some` for a
        /// checker-selected `mut`/`ref` ordinary argument with a supported
        /// caller place, or for a shared-read place a borrowing-view result
        /// lends to.
        arg_places: Vec<Option<MirPlace>>,
        /// Like `Call::kwarg_places`, aligned with `kwargs`.
        kwarg_places: Vec<Option<MirPlace>>,
        capture_accesses: Vec<MirCaptureAccess>,
        /// Compile-time arguments supplied by direct parameterized-method
        /// syntax, in source order. Callable/scalar value parameters retain a
        /// runtime register; type parameters occupy an erased slot.
        param_arg_regs: Vec<MirParamArg>,
        /// Checker-selected generic method contract. This prevents lowering
        /// from reconstructing a bound-method type from the source member.
        param_decls: Vec<ParamDecl>,
        /// The method's own compile-time arguments the checker solved, in
        /// declaration order, as `Call::instantiated_args` carries a `def`'s:
        /// symbolic only over the caller's own binders. A binder no runtime
        /// argument spells (`s.name[Int]()`) is bound from here by the
        /// elaborator; a resolved call keeps none.
        instantiated_args: Vec<TyArg>,
        /// Like `Call::spread`: the position in `args` of a whole type pack
        /// spread into the method's collector (`Sink().take(*args)`), whose
        /// register holds the caller's collector and whose `arg_places`
        /// entry retains the collector's place when the pack is read. The
        /// elaborator replaces it with the bound pack's element places;
        /// concrete MIR carries none.
        spread: Option<usize>,
    },
    /// Move an initialized element from compiler-private `UnsafePointer`
    /// collection storage. The source slot becomes uninitialized, so subsequent
    /// reads/takes/destroys are invalid until an explicit store initializes it.
    PointerStorageTake {
        dest: Reg,
        pointer: Reg,
        index: Reg,
        element: Ty,
    },
    /// Destroy an initialized element in compiler-private `UnsafePointer`
    /// collection storage and mark its slot uninitialized.
    PointerStorageDestroy {
        dest: Reg,
        pointer: Reg,
        index: Reg,
        element: Ty,
    },
    /// Construct compiler-private inline possibly-uninitialized storage
    /// (`MaybeUninit`'s field): uninitialized when `init` is absent,
    /// holding the moved payload otherwise.
    UninitStorage {
        dest: Reg,
        init: Option<Reg>,
    },
    /// Move the payload out of consumed inline uninit storage. Traps if the
    /// storage is uninitialized (upstream UB, deterministic here).
    UninitStorageTake {
        dest: Reg,
        storage: Reg,
        element: Ty,
    },
    /// Destroy the payload of consumed inline uninit storage (runs the
    /// element's destructor). Traps if the storage is uninitialized.
    UninitStorageDestroy {
        dest: Reg,
        storage: Reg,
        element: Ty,
    },
    /// Struct/field *read* `base.field` inside an rvalue (name-based; the backend
    /// resolves layout). Field/index *writes* go through `Store`/a `MirPlace`.
    GetField {
        dest: Reg,
        base: Reg,
        field: String,
    },
    /// Subscript *read* `base[index]` inside an rvalue. Nominal collection
    /// subscripts carry their complete checker-selected invocation in `call`.
    /// This includes the exact `__getitem__` target, effects, parameter access,
    /// compile-time arguments, captures, and any reference-return contract; the
    /// backend never re-derives overload resolution or borrowing semantics.
    Index {
        dest: Reg,
        base: Reg,
        index: Reg,
        /// Stable receiver storage retained by checking/lowering. Nominal
        /// `__getitem__` dispatch uses this for a `ref self` handle and for any
        /// reference returned into the caller's frame.
        base_place: Option<Box<MirPlace>>,
        index_place: Option<Box<MirPlace>>,
        call: Option<Box<MirSubscriptCall>>,
        intrinsic: Option<MirIntrinsicSubscript>,
    },
    /// Slice `object[lower:upper:step]` → a new value. Each bound is
    /// optional (absent = a direction-aware default).
    Slice {
        dest: Reg,
        object: Reg,
        kind: mojito_types::types::SliceKind,
        lower: Option<Reg>,
        upper: Option<Reg>,
        step: Option<Reg>,
        object_place: Option<Box<MirPlace>>,
        arg_places: Vec<Option<MirPlace>>,
        call: Option<Box<MirSubscriptCall>>,
        intrinsic: Option<MirIntrinsicSubscript>,
    },
    /// `object[a, b:c]`: variadic `__getitem__` dispatch with every slice
    /// descriptor selected by the checker and constructed explicitly by the VM.
    MultiIndex {
        dest: Reg,
        object: Reg,
        args: Vec<MirSubscriptArg>,
        object_place: Option<Box<MirPlace>>,
        arg_places: Vec<Option<MirPlace>>,
        /// Keyword subscript actuals — index values (`s[byte=i]`) or slice
        /// descriptors (`s[byte=a:b]`) — bound to keyword-only `__getitem__`
        /// parameters through the checked contract's keyword sources,
        /// parallel to `kwarg_places`.
        kwargs: Vec<(String, MirSubscriptArg)>,
        kwarg_places: Vec<Option<MirPlace>>,
        call: Option<Box<MirSubscriptCall>>,
    },
    /// `object[a, b:c] = value`: checked `__setitem__` dispatch. The receiver
    /// place is retained so a `mut self` implementation is written back after
    /// the call. Variadic setitem methods receive `value` in their keyword-only
    /// slot; fixed-arity methods receive it as the last positional argument.
    MultiSet {
        receiver: Reg,
        receiver_place: Option<Box<MirPlace>>,
        args: Vec<MirSubscriptArg>,
        arg_places: Vec<Option<MirPlace>>,
        value: Reg,
        value_place: Option<Box<MirPlace>>,
        value_keyword: bool,
        call: Box<MirSubscriptCall>,
    },
    /// `place = src` — a write through a place (`p.x = e`, `xs[i] = e`, nested).
    Store {
        place: MirPlace,
        src: Reg,
    },
    /// Initialize reference-valued storage with a reference handle.  Ordinary
    /// `Store` on the same typed place writes through the established handle.
    StoreRef {
        place: MirPlace,
        reference: Reg,
    },
    /// Read a place into a register — for a read-modify-write (`place OP= e`),
    /// where the place (and its indices) must be evaluated exactly once.
    LoadPlace {
        dest: Reg,
        place: MirPlace,
    },
    /// Construct the compiler-private heterogeneous pack-storage primitive.
    /// Source Tuple/List/Set/Dict values are nominal structs and therefore use
    /// ordinary `Call`/`MethodCall` instructions instead of aggregate opcodes.
    MakeTuple {
        dest: Reg,
        elems: Vec<Reg>,
        /// Resolved element types when available. Typed `Tuple[T, ...](...)`
        /// construction uses these to materialize each argument precisely.
        element_types: Option<Vec<Ty>>,
    },
    /// Construct a tagged union. Alternative order determines the runtime tag.
    MakeVariant {
        dest: Reg,
        alternatives: Vec<Ty>,
        index: mojito_types::types::VariantIndex,
        value: Reg,
    },
    /// Test a tag selected during semantic checking.
    VariantIs {
        dest: Reg,
        variant: Reg,
        index: mojito_types::types::VariantIndex,
    },
    /// Extract the active alternative, trapping on a tag mismatch.
    VariantGet {
        dest: Reg,
        variant: Reg,
        index: mojito_types::types::VariantIndex,
    },
    /// Replace a writable variant and destroy its previous payload.
    VariantSet {
        dest: Reg,
        place: MirPlace,
        index: mojito_types::types::VariantIndex,
        value: Reg,
    },
    /// Move a payload out of an already-consumed variant value.
    VariantTake {
        dest: Reg,
        variant: Reg,
        index: mojito_types::types::VariantIndex,
        checked: bool,
    },
    /// In-place placement replacement (`set(init_with=…)`): invoke the
    /// zero-parameter `factory` callable value, store its result as the
    /// payload selected by `index`, and destroy the previous payload.
    VariantSetInitWith {
        dest: Reg,
        place: MirPlace,
        index: mojito_types::types::VariantIndex,
        factory: Reg,
    },
    /// Consuming teardown (`deinit_with`): destructure the moved variant
    /// value and invoke the single-parameter consuming `handler` callable
    /// value with the active payload. The handler covers exactly the
    /// alternative at `index`; any other runtime tag aborts.
    VariantDeinitWith {
        dest: Reg,
        variant: Reg,
        handler: Reg,
        index: mojito_types::types::VariantIndex,
    },
    /// Replace the active payload without destroying it, returning ownership of
    /// that previous payload to the caller.
    VariantReplace {
        dest: Reg,
        place: MirPlace,
        input_index: mojito_types::types::VariantIndex,
        output_index: mojito_types::types::VariantIndex,
        value: Reg,
        checked: bool,
    },
    /// SIMD construction `SIMD[DType.<dt>, width](elems)` (or a scalar-alias like
    /// `Int32(x)`). The element `dtype`/`width` are the construction type's
    /// slots: known, or in a generator the parameter expressions the body
    /// names (`Scalar[dt](x)`, `SIMD[dt, 2 * n](x)`), which the elaborator
    /// closes per instance; concrete MIR holds known slots. `elems` are the
    /// lane values (exactly `width`, or one to splat).
    MakeSimd {
        dest: Reg,
        dtype: mojito_types::types::SimdDtype,
        width: mojito_types::types::SimdWidth,
        elems: Vec<Reg>,
    },
    /// Elementwise dtype conversion `v.cast[DType.<dt>]()`. The target
    /// `dtype`/`width` are the result type's slots, like
    /// [`MirInstr::MakeSimd`]'s.
    SimdCast {
        dest: Reg,
        value: Reg,
        dtype: mojito_types::types::SimdDtype,
        width: mojito_types::types::SimdWidth,
    },
    /// Lane-wise bit reinterpretation `v.to_bits[DType.<dt>]()`: each lane's
    /// bit pattern zero-extended into the unsigned target `dtype` (at least
    /// as wide as the source lane; `bool` reads as 0/1). The target
    /// `dtype`/`width` are the result type's slots, like
    /// [`MirInstr::MakeSimd`]'s.
    SimdBitcast {
        dest: Reg,
        value: Reg,
        dtype: mojito_types::types::SimdDtype,
        width: mojito_types::types::SimdWidth,
    },
    /// Lane gather `v.shuffle[*mask]()`, `v.slice[...]()`, or `v.join(w)`:
    /// result lane `i` is lane `mask[i]` of `value`'s lanes followed by
    /// `other`'s, which only a join has (`shufflevector`'s second operand,
    /// of `value`'s own type). The mask is a compile-time parameter: a
    /// known one has every index within those lanes and a valid SIMD width
    /// as its length; a template's names its binders, and the elaborator
    /// closes it per instance. Concrete MIR holds only a known mask.
    SimdShuffle {
        dest: Reg,
        value: Reg,
        other: Option<Reg>,
        mask: mojito_types::types::LaneMask,
    },
    /// `raise <src>` — raise an error value. Propagates as an exceptional outcome
    /// (the VM unwinds to the nearest enclosing [`MirInstr::Try`] handler).
    Raise {
        src: Reg,
    },
    /// A structurally lowered `try`/`except`/`else`/`finally` region. Each
    /// sub-part is a self-contained mini-CFG (a
    /// `Vec<MirBlock>` with local block ids, entry = block 0) that **shares this
    /// function's register and variable space** — so it addresses the same slots.
    /// `handler` is `Some((error_var, body))` when there is an `except` clause (the
    /// optional slot binds the caught error). `cleanup` lists the body-local
    /// variables to drop when the body unwinds (the exceptional-edge cleanup).
    Try {
        body: Vec<MirBlock>,
        handler: Option<(Option<VarId>, Vec<MirBlock>)>,
        orelse: Option<Vec<MirBlock>>,
        finalbody: Option<Vec<MirBlock>>,
        cleanup: Vec<VarId>,
    },
    /// `hlcf.if`: run `then` when `cond` holds; otherwise try each `elifs`
    /// arm in order, running its `cond` region (ended by
    /// [`MirTerm::ElifCond`]) and its `then` when that holds; otherwise run
    /// `orelse`. An arm that completes ends in [`MirTerm::Yield`], whose
    /// values define `results`. Regions share the function's registers and
    /// slots, with local block ids like a [`MirInstr::Try`] part's; the
    /// structured form holds each as one block. Concrete MIR carries none:
    /// [`super::lower_control_flow`] lowers it first.
    If {
        results: Vec<Reg>,
        cond: Reg,
        then: Vec<MirBlock>,
        elifs: Vec<MirElif>,
        orelse: Vec<MirBlock>,
    },
    /// `hlcf.loop`: run `body` until a [`MirTerm::Break`] leaves it;
    /// [`MirTerm::Continue`] starts the next iteration. `label` names the
    /// loop for a labelled `break`/`continue`, which targets the nearest
    /// enclosing loop carrying it.
    Loop {
        label: Option<LoopLabel>,
        body: Vec<MirBlock>,
    },
    /// `hlcf.comptime.if`: run the arm of the first condition of `conds`
    /// that holds, else `orelse`, the conditions being parameter expressions
    /// the elaborator decides. `arms` has one region per condition; an arm
    /// that completes ends in [`MirTerm::ComptimeYield`], whose values
    /// define `results`.
    ComptimeIf {
        results: Vec<Reg>,
        conds: Vec<GenericConstraint>,
        arms: Vec<Vec<MirBlock>>,
        orelse: Vec<MirBlock>,
    },
    /// `hlcf.comptime.for`: run `body` once per iterator the compile-time
    /// sequence `source` steps through, binding it to the parameter binder
    /// `binder`; the loop variable is an alias of `source.next_value` over
    /// it and has no storage. The body is one block holding a
    /// [`MirInstr::ComptimeIf`] on [`ComptimeSequence::stop_test`] and ending
    /// in [`MirTerm::Unreachable`]: the arm is the suite, which ends an
    /// iteration with [`MirTerm::ComptimeContinue`], and the else arm is the
    /// stop arm, which leaves the loop with [`MirTerm::ComptimeBreak`], as a
    /// `break` in the suite does. Nothing continues after the op but a
    /// break. `orelse` is upstream's else region after `LowerSemanticCF`:
    /// one block holding [`MirTerm::Unreachable`].
    ///
    /// [`ComptimeSequence::stop_test`]: mojito_checked::checked::ComptimeSequence::stop_test
    ComptimeFor {
        binder: mojito_types::param_expr::ParamRef,
        source: mojito_checked::checked::ComptimeSequence,
        body: Vec<MirBlock>,
        orelse: Vec<MirBlock>,
    },
    /// An ASAP destructor on a register (reserved for the future Op/assembler VM).
    Drop {
        reg: Reg,
    },
    /// Drop the value in a variable slot — spliced in by the Stage 7 liveness pass
    /// at a variable's last use (ASAP destruction). Runs the value's `__deinit__` (and
    /// its fields', in reverse order) and leaves the slot empty. A no-op for values
    /// without a destructor, so it never changes observable behaviour except when a
    /// struct defines `__deinit__`.
    DropVar {
        var: VarId,
    },
    /// Consume a variable without running implicit destruction. Explicit-destroy
    /// calls emit this immediately after the call, so a raising call leaves the
    /// source live for an `except` fallback while a successful call consumes it.
    ConsumeVar {
        var: VarId,
    },
    /// Consume one projected subobject after its named explicit destructor
    /// succeeds, leaving the rest of the aggregate available.
    ConsumePlace {
        place: MirPlace,
        marker: Reg,
    },
    /// Mark `place` initialized without writing it (upstream's
    /// `lit.ownership.mark_initialized`): ownership treats the place as
    /// defined from here, so a constructor may fill its storage through
    /// pointers; the storage itself already exists, so the backends emit
    /// nothing.
    MarkInitialized {
        place: MirPlace,
    },
    /// End `place`'s value without destroying it (upstream's
    /// `lit.ownership.mark_destroyed`): ownership treats the place as moved
    /// from here, its storage having been moved out through pointers, so no
    /// destructor runs on it.
    MarkDestroyed {
        place: MirPlace,
    },
    /// Destroy one projected field of an aggregate now — running the field
    /// value's own destructor — and tombstone it, leaving the rest of the
    /// aggregate live. Drop elaboration emits this for a `deinit`
    /// parameter's direct fields, each at its own last use (the receiver's
    /// later `ConsumeVar` skips the tombstoned fields), and immediately
    /// before a `Store` that overwrites an initialized droppable sub-place,
    /// so the replaced value is destroyed at the assignment.
    DropPlace {
        place: MirPlace,
    },
    /// A construct the MIR/backends don't lower yet (a `try` with its exceptional
    /// edges, a nested declaration). Kept as an explicit node — rather than a
    /// lowering-time `panic!` — so a backend can report a clean error instead of
    /// crashing on an otherwise-valid program.
    Unsupported(String),
    /// Iterator protocol: normalize `source` through its checker-selected nominal
    /// `__iter__()` implementation, producing the iterator in `dest`. When
    /// `source == dest` the iterable is normalized in place; when they differ,
    /// `source` retains the live iterable in its own slot (dropped after the loop)
    /// so a borrowing iterator does not clobber its only owner.
    GetIter {
        source: VarId,
        dest: VarId,
        mode: mojito_checked::checked::IterationMode,
        prepare: Vec<String>,
    },
    /// Iterator protocol over compiler-private iterator storage (a runtime
    /// pack or a compile-time list): read whether the iterator variable `iter`
    /// yields another element into `dest` (a `Bool`) — a pure read. Nominal
    /// iterators advance through [`MirInstr::TryNext`] instead.
    HasNext {
        dest: Reg,
        iter: VarId,
    },
    /// Iterator protocol over compiler-private iterator storage: bind the
    /// current element into `dest` and advance the iterator variable `iter`
    /// in place (a mutating read).
    Next {
        dest: Reg,
        iter: VarId,
    },
    /// Invoke a typed-raising iterator `__next__`. `yielded` is true when
    /// `dest` contains an element and false when the call raises exactly the
    /// checked `exhaustion` type; other raised values propagate.
    TryNext {
        dest: Reg,
        yielded: Reg,
        iter: VarId,
        /// Exact selected target, reference/value ABI, and raising effect.
        call: Box<mojito_checked::checked::CheckedIteratorCall>,
        exhaustion: Ty,
    },
}

/// An instruction is sized for its common variants: the places and subscript
/// calls only a few call variants carry sit behind boxes, and a type is one
/// handle. The call variants' argument lists set this bound.
const _: () = assert!(std::mem::size_of::<MirInstr>() <= 296);

impl MirInstr {
    /// The checked error type a call instruction raises, if it raises.
    pub fn call_error(&self) -> Option<&Ty> {
        match self {
            Self::Call { raises, .. }
            | Self::CallIndirect { raises, .. }
            | Self::MethodCall { raises, .. } => raises.as_ref(),
            Self::Index { call, .. } | Self::Slice { call, .. } | Self::MultiIndex { call, .. } => {
                call.as_ref().and_then(|call| call.raises.as_ref())
            }
            Self::MultiSet { call, .. } => call.raises.as_ref(),
            Self::UnOp { raises, .. } | Self::BinOp { raises, .. } => raises.as_ref(),
            _ => None,
        }
    }

    /// Whether the instruction is a raise point: a `raise`, or a call
    /// declared `raises`.
    pub fn is_raise_point(&self) -> bool {
        matches!(self, Self::Raise { .. }) || self.call_error().is_some()
    }

    /// Whether the instruction has no effect but reading, so it can be
    /// erased once nothing reads its results: MLIR's
    /// `wouldOpBeTriviallyDead`, which upstream's `Canonicalizer` asks.
    ///
    /// As in MLIR, an instruction that does not declare itself free of
    /// effects has them. These stay:
    /// - a copy (`var.use` in copy mode, `value.copy`): it may run
    ///   `__copyinit__`, which only an instance decides; a `value.copy` that
    ///   is the identity is folded instead ([`Self::forwarded_operand`]);
    /// - a move, forward, or place move: each ends a slot;
    /// - an operator: a generic operand's becomes a dunder call per instance,
    ///   and built-in division traps;
    /// - a tuple, variant, or closure construction: it takes or copies owned
    ///   operands; a type construction runs an initializer;
    /// - `rebind`, a place read through an index, variant, uninit-payload, or
    ///   rebind projection: each calls `__getitem__`, traps, or is an
    ///   assertion the elaborator judges per instance;
    /// - every call, store, drop, consume, ownership mark, loan or interior
    ///   marker, keep-alive, scope start, iterator step, `raise`, region op,
    ///   and unsupported instruction.
    pub fn would_be_trivially_dead(&self) -> bool {
        let reads_storage = |place: &MirPlace| {
            place
                .proj
                .iter()
                .all(|step| matches!(step, Proj::Field(_) | Proj::ConstIndex(_)))
        };
        match self {
            Self::Const { .. }
            | Self::MaterializeLiteral { .. }
            | Self::SizeOf { .. }
            | Self::TypeName { .. }
            | Self::ParamListAddress { .. }
            | Self::GetField { .. }
            | Self::ReadRef { .. }
            | Self::VariantIs { .. }
            | Self::MakeSimd { .. }
            | Self::SimdCast { .. }
            | Self::SimdBitcast { .. }
            | Self::SimdShuffle { .. } => true,
            Self::UseVar { mode, .. } => matches!(mode, UseMode::BorrowShared | UseMode::BorrowMut),
            Self::LoadPlace { place, .. } | Self::MakeRef { place, .. } => reads_storage(place),
            Self::EstablishLoans { .. }
            | Self::InvalidateInteriors { .. }
            | Self::Rebind { .. }
            | Self::CopyValue { .. }
            | Self::WriteRef { .. }
            | Self::MakeClosure { .. }
            | Self::KeepAlive { .. }
            | Self::ConstructTypeParam { .. }
            | Self::ConstructType { .. }
            | Self::MovePlace { .. }
            | Self::DefVar { .. }
            | Self::StoreToMem { .. }
            | Self::DeclareVar { .. }
            | Self::UnOp { .. }
            | Self::BinOp { .. }
            | Self::Call { .. }
            | Self::CallIndirect { .. }
            | Self::MethodCall { .. }
            | Self::PointerStorageTake { .. }
            | Self::PointerStorageDestroy { .. }
            | Self::UninitStorage { .. }
            | Self::UninitStorageTake { .. }
            | Self::UninitStorageDestroy { .. }
            | Self::Index { .. }
            | Self::Slice { .. }
            | Self::MultiIndex { .. }
            | Self::MultiSet { .. }
            | Self::Store { .. }
            | Self::StoreRef { .. }
            | Self::MakeTuple { .. }
            | Self::MakeVariant { .. }
            | Self::VariantGet { .. }
            | Self::VariantSet { .. }
            | Self::VariantTake { .. }
            | Self::VariantSetInitWith { .. }
            | Self::VariantDeinitWith { .. }
            | Self::VariantReplace { .. }
            | Self::Raise { .. }
            | Self::Try { .. }
            | Self::If { .. }
            | Self::Loop { .. }
            | Self::ComptimeIf { .. }
            | Self::ComptimeFor { .. }
            | Self::Drop { .. }
            | Self::DropVar { .. }
            | Self::ConsumeVar { .. }
            | Self::ConsumePlace { .. }
            | Self::MarkInitialized { .. }
            | Self::MarkDestroyed { .. }
            | Self::DropPlace { .. }
            | Self::Unsupported(_)
            | Self::GetIter { .. }
            | Self::HasNext { .. }
            | Self::Next { .. }
            | Self::TryNext { .. } => false,
        }
    }

    /// The operand this instruction's result always equals, when it folds to
    /// one: MLIR's `fold` returning an operand, after which upstream's
    /// `Canonicalizer` replaces the result's uses with it.
    ///
    /// Only a `value.copy` folds, and only when its source and destination are
    /// typed alike by a type whose copy is the identity
    /// ([`MirDeclarations::copy_is_identity`]): a one-register value, or a
    /// register-passable trivial one, which upstream's parser loads into an
    /// SSA value its copies are. A widening copy changes its readers'
    /// operand type, and a `rebind` is an assertion the elaborator judges
    /// per instance.
    pub fn forwarded_operand(
        &self,
        reg_types: &HashMap<u32, Ty>,
        declarations: &MirDeclarations,
    ) -> Option<Reg> {
        let Self::CopyValue { dest, value } = self else {
            return None;
        };
        let source = reg_types.get(&value.0)?;
        (reg_types.get(&dest.0) == Some(source) && declarations.copy_is_identity(source))
            .then_some(*value)
    }

    /// The operand and target of a built-in scalar conversion
    /// (`Int(x)`, `UInt(x)`, `Float64(x)`, `Bool(x)`): a `call` of the bare
    /// builtin name that `declarations` declares no function under, with one
    /// positional argument and nothing else, which the VM's `call_named` and
    /// Pliron's `lower_call` treat as the cast upstream's inlined
    /// `SIMD.__init__`/`Bool.__init__` leave behind.
    pub fn scalar_conversion(
        &self,
        declarations: &MirDeclarations,
    ) -> Option<(Reg, ScalarConversion)> {
        let Self::Call {
            func,
            raises: None,
            args,
            kwargs,
            param_arg_regs,
            receiver: None,
            instantiated_args,
            spread: None,
            ..
        } = self
        else {
            return None;
        };
        let [arg] = args.as_slice() else {
            return None;
        };
        let builtin = declarations.function(&func.0).is_none();
        (builtin && kwargs.is_empty() && param_arg_regs.is_empty() && instantiated_args.is_empty())
            .then(|| ScalarConversion::named(&func.0))
            .flatten()
            .map(|target| (*arg, target))
    }

    /// The constant this instruction's one result always equals when its
    /// operands hold the constants `constant` gives: MLIR's `fold` returning
    /// an attribute, which upstream's `SCCP` asks.
    ///
    /// Only a built-in operator, a literal materialization to `Int`, `UInt`,
    /// `Float64`, or a one-lane SIMD type, a one-lane `simd.make` of a
    /// literal, a wider `simd.make` with one element (upstream's splat
    /// `SIMD.__init__`) or one element per lane (upstream's
    /// `SIMD.__init__(*elems: Scalar[dtype])`), each a literal, a one-lane
    /// constant of its dtype, or a `Bool` into a `bool` vector, a built-in scalar conversion
    /// ([`Self::scalar_conversion`], upstream's inlined `pop.cast`), and a
    /// forwarded copy fold, and only over values
    /// whose constant kind is the one their register's type names. A
    /// materialization or a one-lane `simd.make` folds to the lane the VM
    /// materializes (`CtValue::materialize_as`), a wider one to the lanes
    /// `ct::simd_construction_lanes` gives (the comptime evaluator's own
    /// construction rule), a `UInt` or SIMD one to a
    /// `Const::Value`.
    /// An operator over `UInt` or SIMD lanes folds by the VM's own lane rule
    /// (`mojito_types::ct::int_lane_binop` and its siblings), and a lane
    /// comparison folds to a `bool` mask. The fold is the compile-time
    /// folder's; where it and the VM could disagree (a zero scalar divisor,
    /// an out-of-range shift, an overflowing `**`, a NaN or infinite scalar
    /// comparison, `Float64` `//`, `%`, `**`, float-lane `//`, `%`, `**`, and
    /// `Bool` operators other than `==`/`!=`, a float converted to `Int` or
    /// `UInt` out of range or not finite), nothing folds and the run-time
    /// operation stays. A scalar conversion folds by the VM's own rule
    /// (`CtValue::folded_conversion`).
    pub fn folded_constant(
        &self,
        constant: impl Fn(Reg) -> Option<Const>,
        reg_types: &HashMap<u32, Ty>,
        declarations: &MirDeclarations,
    ) -> Option<Const> {
        use mojito_types::param_expr::fold::{fold_infix, fold_invert, fold_neg};
        let operand = |reg: Reg| const_ct_value(&constant(reg)?, reg_types.get(&reg.0)?);
        let (dest, value) = match self {
            Self::BinOp {
                op,
                dest,
                a,
                b,
                resolved: None,
                raises: None,
            } => {
                let (left, right) = (operand(*a)?, operand(*b)?);
                let bool_operands = matches!(left, CtValue::Bool(_));
                if bool_operands && !matches!(op, InfixOp::Eq | InfixOp::Ne) {
                    return None;
                }
                (dest, fold_infix(*op, &left, &right).ok()?)
            }
            Self::UnOp {
                op,
                dest,
                a,
                raises: None,
            } => {
                let value = operand(*a)?;
                let folded = match (op, value) {
                    (PrefixOp::Neg, value) => fold_neg(&value).ok()?,
                    (PrefixOp::Invert, value) => fold_invert(&value).ok()?,
                    (PrefixOp::Not, CtValue::Bool(value)) => CtValue::Bool(!value),
                    (PrefixOp::Not, _) => return None,
                };
                (dest, folded)
            }
            Self::MaterializeLiteral {
                dest,
                value,
                target,
            } => {
                let literal = operand(*value)?;
                let materializes =
                    matches!(target.kind(), TyKind::Int | TyKind::UInt | TyKind::Float64)
                        || mojito_types::types::scalar_simd_dtype(target.kind()).is_some();
                if !materializes
                    || !matches!(literal, CtValue::IntLiteral(_) | CtValue::FloatLiteral(_))
                {
                    return None;
                }
                let materialized = literal.materialize_as(target)?;
                (dest, materialized)
            }
            Self::MakeSimd {
                dest,
                dtype: mojito_types::types::SimdDtype::Known(dtype),
                width: mojito_types::types::SimdWidth::Known(width),
                elems,
            } => {
                let width = usize::try_from(*width).ok().filter(|width| *width > 0)?;
                let values = elems
                    .iter()
                    .map(|element| operand(*element))
                    .collect::<Option<Vec<_>>>()?;
                let folded = match values.as_slice() {
                    [literal @ (CtValue::IntLiteral(_) | CtValue::FloatLiteral(_))]
                        if width == 1 =>
                    {
                        literal.clone().materialize_as(reg_types.get(&dest.0)?)?
                    }
                    _ if width == 1 => return None,
                    _ if values.iter().all(|value| is_scalar_element(value, *dtype)) => {
                        CtValue::Simd {
                            dtype: *dtype,
                            lanes: mojito_types::ct::simd_construction_lanes(
                                *dtype, width, &values,
                            )
                            .ok()?,
                        }
                    }
                    _ => return None,
                };
                (dest, folded)
            }
            Self::CopyValue { dest, .. } => (
                dest,
                operand(self.forwarded_operand(reg_types, declarations)?)?,
            ),
            Self::Call { dest, .. } => {
                let (arg, target) = self.scalar_conversion(declarations)?;
                (dest, operand(arg)?.folded_conversion(target)?)
            }
            _ => return None,
        };
        ct_const(value, reg_types.get(&dest.0)?)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MirCaptureMode {
    Reference,
    Copy,
    Move,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MirClosureCapture {
    pub place: MirPlace,
    pub mode: MirCaptureMode,
}

/// A static access to owner storage performed by a callable environment while
/// executing a call. Origin paths deliberately retain abstract indices; the VM
/// never interprets this metadata.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MirCaptureAccess {
    pub root: VarId,
    pub path: Vec<mojito_types::origin::OriginSeg>,
    pub access: mojito_types::origin::CaptureAccess,
}

/// How a basic block hands off control. Block targets are indices into
/// `MirFunction::blocks`; values are registers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum MirTerm {
    Jump(MirBlockId),
    Branch {
        cond: Reg,
        then_b: MirBlockId,
        else_b: MirBlockId,
    },
    /// A `comptime if` arm test, as `lower-control-flow` lowers a
    /// [`MirInstr::ComptimeIf`]: a branch whose condition is a parameter
    /// expression over the function's binders, decided by the elaborator, which keeps
    /// the taken arm. Ownership and drops treat it as `Branch` with the
    /// condition opaque; concrete MIR carries none.
    ComptimeBranch {
        cond: Box<GenericConstraint>,
        then_b: MirBlockId,
        else_b: MirBlockId,
    },
    /// A `comptime for` header: a loop whose binder `binder` takes each
    /// iterator the compile-time sequence `source` steps through; the loop
    /// variable is an alias of `source.next_value` over it and has no
    /// storage. The body's back edge jumps here; `exit` follows the loop,
    /// where a `break` and the body's stop arm land. Ownership and drops
    /// treat it as a loop with its trip count unknown; the elaborator unrolls
    /// it, so concrete MIR carries none.
    ComptimeFor {
        binder: mojito_types::param_expr::ParamRef,
        source: mojito_checked::checked::ComptimeSequence,
        body: MirBlockId,
        exit: MirBlockId,
    },
    Return(Option<Reg>),
    /// Return after evaluating `value`, carrying structured loop-owned cleanup
    /// out through any enclosing `try/finally` regions. The VM performs these
    /// drops only after every pending `finally` has run.
    ReturnWithCleanup {
        value: Option<Reg>,
        cleanup: Vec<VarId>,
    },
    /// `lit.try.yield`: a `try` part completed (see
    /// [`hir::Terminator::TryYield`]), so the `try` goes on to its
    /// `else`/`finally`. Never appears in a function body's blocks.
    TryYield,
    /// A `break`/`continue` inside a `try` region that targets an enclosing
    /// **function** loop: `target` is that loop's exit/header block in the
    /// *enclosing function*'s `blocks` (not the region's). The VM propagates it out
    /// as a `Flow::Jump(target)` — running each `finally` on the way — until the
    /// function driver jumps there. `cleanup` lists the region-body-local variables
    /// to drop when this escape edge is taken (filled by drop elaboration).
    EscapeJump {
        target: MirBlockId,
        cleanup: Vec<VarId>,
    },
    /// `hlcf.yield`: end an [`MirInstr::If`] arm, its values defining the
    /// op's results.
    Yield(Vec<Reg>),
    /// `hlcf.if.elifcond.yield`: end an elif condition region with the
    /// `Bool` that selects its arm.
    ElifCond(Reg),
    /// `hlcf.break`: leave the nearest enclosing [`MirInstr::Loop`], or
    /// the nearest one carrying `label`.
    Break {
        label: Option<LoopLabel>,
    },
    /// `hlcf.continue`: start the next iteration of the loop
    /// [`MirTerm::Break`] would leave.
    Continue {
        label: Option<LoopLabel>,
    },
    /// `hlcf.comptime.yield`: end a [`MirInstr::ComptimeIf`] arm, its
    /// values defining the op's results.
    ComptimeYield(Vec<Reg>),
    /// `hlcf.comptime.for.break`: leave the nearest enclosing
    /// [`MirInstr::ComptimeFor`].
    ComptimeBreak,
    /// `hlcf.comptime.for.continue`: end this iteration of the nearest
    /// enclosing [`MirInstr::ComptimeFor`].
    ComptimeContinue,
    /// `hlcf.unreachable`: control never reaches here.
    Unreachable,
}

/// One `elif` arm of a [`MirInstr::If`]: the region computing its
/// condition, ended by [`MirTerm::ElifCond`], and the arm it selects.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MirElif {
    pub cond: Vec<MirBlock>,
    pub then: Vec<MirBlock>,
}

/// The label of a [`MirInstr::Loop`]: upstream's string attribute, spelled
/// as a number. Labels need not be unique; a labelled `break`/`continue`
/// targets the nearest enclosing loop carrying it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct LoopLabel(pub u32);

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MirBlock {
    pub instrs: Vec<MirInstr>,
    pub term: MirTerm,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MirFunction {
    pub blocks: Vec<MirBlock>,
    pub n_regs: u32,
    /// Number of variable slots (the interner size). A frame allocates this many
    /// var cells; `UseVar`/`DefVar` index into them.
    pub n_vars: usize,
    /// The name of each variable slot (`var_names[id]` is `VarId` `id`'s source
    /// name; synthetic `$…` names for compiler temporaries). For diagnostics — the
    /// ownership analysis names the offending variable.
    pub var_names: Vec<String>,
    /// Number of leading vars that are parameters (`vars[0..n_params]`), bound from
    /// the call's arguments in declaration order — the VM call ABI.
    pub n_params: usize,
    /// The checker-resolved type of each parameter (same order/length as the
    /// params), used when binding arguments. Empty for `__toplevel__`.
    pub param_types: Vec<Ty>,
    /// Whether each parameter is consuming (the callee takes ownership, so it drops
    /// the value — unlike a borrowed `read`/`mut` parameter). Same order as the
    /// params; the caller transfers with `^`, so its own drop is skipped.
    pub owned_params: Vec<bool>,
    /// Whether each parameter is a `deinit` (destructor/move-source) parameter.
    /// Same order/length as the params. Such a parameter is *consumed* at its
    /// drop point — its residual fields are destroyed but its whole-value
    /// `__deinit__` is skipped (its resources moved into the receiver), so drop
    /// elaboration lowers its teardown to `ConsumeVar` instead of `DropVar`.
    pub deinit_params: Vec<bool>,
    /// Whether each parameter is a `mut`/`ref` **reference** (its final value is
    /// written back to the caller). Same order as the params, `self` included:
    /// a `mut self` receiver is parameter 0 and is flagged here.
    pub ref_params: Vec<bool>,
    pub returns_reference: bool,
    /// Checked type of each variable slot, as far as lowering recorded one.
    /// Parameters, bindings, and synthetic locals are covered on the checked
    /// path; instruction typing and verification read slot types from here.
    #[serde(serialize_with = "super::sorted_map")]
    pub var_tys: HashMap<VarId, Ty>,
    /// Checked return type. `None` only on unchecked compatibility paths;
    /// production lowering always records it (`TyKind::None` for no return).
    pub ret_ty: Option<Ty>,
    /// Checked raising contract (`raises Never` records as nonraising).
    pub raises: bool,
    /// Declared error type when `raises` is true and the contract is typed.
    pub error_ty: Option<Ty>,
    /// Not encoded: a package writes its bodies' locations against its own
    /// file table.
    #[serde(skip)]
    pub spans: SpanTable,
    /// Resolved type of registers originating in checked expressions. Synthetic
    /// control-flow registers are filled by instruction typing before verification.
    #[serde(serialize_with = "super::sorted_map")]
    pub reg_types: HashMap<u32, Ty>,
    /// The function's own compile-time parameters nothing in its body or
    /// signature reads, sorted by slot: upstream's `RemoveUnusedParams`
    /// result, which the elaborator leaves out of an instance's identity.
    /// Not encoded: a package holds its bodies before the pipeline that
    /// computes it. Empty on a concrete instance.
    #[serde(skip)]
    pub removed_params: Vec<mojito_types::param_expr::ParamId>,
}

impl MirFunction {
    /// The instructions in this body's blocks, terminators not counted.
    pub fn instruction_count(&self) -> usize {
        self.blocks.iter().map(|block| block.instrs.len()).sum()
    }

    /// The blocks of this body, its `try` regions' not counted.
    pub const fn block_count(&self) -> usize {
        self.blocks.len()
    }
}

/// Maps each generated register to its source span and (if it names one) the
/// origin variable — so borrow-checker diagnostics can point at real code.
#[derive(Debug, Clone, Default)]
pub struct SpanTable(pub HashMap<u32 /*reg*/, (SourceSpan, Option<VarId>)>);

/// The scalar `constant` denotes in a register of type `ty`, when its kind
/// is the one `ty` names.
fn const_ct_value(constant: &Const, ty: &Ty) -> Option<CtValue> {
    match (constant, ty.kind()) {
        (Const::Int(value), TyKind::Int) => Some(CtValue::Int(*value)),
        (Const::Float(value), TyKind::Float64) => Some(CtValue::Float(value.to_bits())),
        (Const::Bool(value), TyKind::Bool) => Some(CtValue::Bool(*value)),
        (Const::IntLiteral(value), TyKind::IntLiteral) => Some(CtValue::IntLiteral(value.clone())),
        (Const::FloatLiteral(value), TyKind::FloatLiteral) => {
            Some(CtValue::FloatLiteral(value.clone()))
        }
        (Const::Value(value @ CtValue::UInt(_)), TyKind::UInt) => Some(value.clone()),
        (
            Const::Value(value @ CtValue::Simd { dtype, lanes }),
            TyKind::Simd {
                dtype: mojito_types::types::SimdDtype::Known(own),
                width: mojito_types::types::SimdWidth::Known(width),
            },
        ) if own == dtype && usize::try_from(*width) == Ok(lanes.len()) => Some(value.clone()),
        _ => None,
    }
}

/// Whether `value` is an element upstream's `SIMD.__init__(*elems:
/// Scalar[dtype])` admits: a literal, a `Bool` into a `bool` vector, or a
/// one-lane value of the vector's own dtype. Mojito's narrowing of other
/// elements (divergence R186) never folds.
fn is_scalar_element(value: &CtValue, dtype: Dtype) -> bool {
    match value {
        CtValue::IntLiteral(_) | CtValue::FloatLiteral(_) => true,
        CtValue::Bool(_) => dtype == Dtype::Bool,
        CtValue::Simd { dtype: own, lanes } => *own == dtype && lanes.len() == 1,
        _ => false,
    }
}

/// The constant a register of type `ty` holding the scalar `value` is
/// defined by, when `ty` names `value`'s kind.
fn ct_const(value: CtValue, ty: &Ty) -> Option<Const> {
    match (value, ty.kind()) {
        (CtValue::Int(value), TyKind::Int) => Some(Const::Int(value)),
        (CtValue::Float(bits), TyKind::Float64) => Some(Const::Float(f64::from_bits(bits))),
        (CtValue::Bool(value), TyKind::Bool) => Some(Const::Bool(value)),
        (CtValue::IntLiteral(value), TyKind::IntLiteral) => Some(Const::IntLiteral(value)),
        (CtValue::FloatLiteral(value), TyKind::FloatLiteral) => Some(Const::FloatLiteral(value)),
        (value @ (CtValue::UInt(_) | CtValue::Simd { .. }), _) => {
            let constant = Const::Value(value);
            const_ct_value(&constant, ty).is_some().then_some(constant)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(instrs: Vec<MirInstr>, term: MirTerm) -> MirBlock {
        MirBlock { instrs, term }
    }

    fn function(blocks: Vec<MirBlock>) -> MirFunction {
        MirFunction {
            blocks,
            n_regs: 0,
            n_vars: 0,
            var_names: Vec::new(),
            n_params: 0,
            param_types: Vec::new(),
            owned_params: Vec::new(),
            deinit_params: Vec::new(),
            ref_params: Vec::new(),
            returns_reference: false,
            var_tys: HashMap::new(),
            ret_ty: None,
            raises: false,
            error_ty: None,
            spans: SpanTable::default(),
            reg_types: HashMap::new(),
            removed_params: Vec::new(),
        }
    }

    #[test]
    fn prune_unreachable_blocks_renumbers_jumps_and_escapes() {
        // bb0 -> bb2 -> bb3; bb1 is the untaken arm. bb2 holds a region
        // whose escape names bb3 by its function-level index.
        let region = vec![block(
            Vec::new(),
            MirTerm::EscapeJump {
                target: 3,
                cleanup: Vec::new(),
            },
        )];
        let mut f = function(vec![
            block(Vec::new(), MirTerm::Jump(2)),
            block(Vec::new(), MirTerm::Return(None)),
            block(
                vec![MirInstr::Try {
                    body: region,
                    handler: None,
                    orelse: None,
                    finalbody: None,
                    cleanup: Vec::new(),
                }],
                MirTerm::Jump(3),
            ),
            block(Vec::new(), MirTerm::Return(None)),
        ]);
        prune_unreachable_blocks(&mut f);
        assert_eq!(f.blocks.len(), 3);
        assert!(matches!(f.blocks[0].term, MirTerm::Jump(1)));
        assert!(matches!(f.blocks[1].term, MirTerm::Jump(2)));
        let MirInstr::Try { body, .. } = &f.blocks[1].instrs[0] else {
            panic!("the region survives with its block");
        };
        assert!(matches!(
            body[0].term,
            MirTerm::EscapeJump { target: 2, .. }
        ));
    }

    #[test]
    fn prune_unreachable_blocks_keeps_a_block_only_an_escape_reaches() {
        let region = vec![block(
            Vec::new(),
            MirTerm::EscapeJump {
                target: 2,
                cleanup: Vec::new(),
            },
        )];
        let mut f = function(vec![
            block(
                vec![MirInstr::Try {
                    body: region,
                    handler: None,
                    orelse: None,
                    finalbody: None,
                    cleanup: Vec::new(),
                }],
                MirTerm::Return(None),
            ),
            block(Vec::new(), MirTerm::Return(None)),
            block(Vec::new(), MirTerm::Return(None)),
        ]);
        prune_unreachable_blocks(&mut f);
        assert_eq!(f.blocks.len(), 2);
        let MirInstr::Try { body, .. } = &f.blocks[0].instrs[0] else {
            panic!("the region survives with its block");
        };
        assert!(matches!(
            body[0].term,
            MirTerm::EscapeJump { target: 1, .. }
        ));
    }

    #[test]
    fn prune_unreachable_blocks_prunes_every_region_from_its_entry() {
        // An `if` arm and a loop body each hold an unreachable block; the
        // loop body holds a `try` whose escape alone reaches bb2.
        let arm = vec![
            block(Vec::new(), MirTerm::Yield(Vec::new())),
            block(Vec::new(), MirTerm::Unreachable),
        ];
        let escape = vec![block(
            Vec::new(),
            MirTerm::EscapeJump {
                target: 2,
                cleanup: Vec::new(),
            },
        )];
        let body = vec![
            block(
                vec![MirInstr::Try {
                    body: escape,
                    handler: None,
                    orelse: None,
                    finalbody: None,
                    cleanup: Vec::new(),
                }],
                MirTerm::Jump(2),
            ),
            block(Vec::new(), MirTerm::Unreachable),
            block(Vec::new(), MirTerm::Break { label: None }),
        ];
        let mut f = function(vec![
            block(
                vec![
                    MirInstr::If {
                        results: Vec::new(),
                        cond: Reg(0),
                        then: arm.clone(),
                        elifs: Vec::new(),
                        orelse: arm,
                    },
                    MirInstr::Loop { label: None, body },
                ],
                MirTerm::Return(None),
            ),
            block(Vec::new(), MirTerm::Return(None)),
            block(Vec::new(), MirTerm::Return(None)),
        ]);
        prune_unreachable_blocks(&mut f);
        assert_eq!(f.blocks.len(), 2);
        let regions = instruction_regions(&f.blocks[0].instrs[0]);
        assert!(regions.iter().all(|region| region.len() == 1));
        let MirInstr::Loop { body, .. } = &f.blocks[0].instrs[1] else {
            panic!("the loop survives");
        };
        assert_eq!(body.len(), 2);
        assert!(matches!(body[0].term, MirTerm::Jump(1)));
        let MirInstr::Try { body: escape, .. } = &body[0].instrs[0] else {
            panic!("the try survives");
        };
        assert!(matches!(
            escape[0].term,
            MirTerm::EscapeJump { target: 1, .. }
        ));
    }

    #[test]
    fn only_an_instruction_free_of_effects_would_be_trivially_dead() {
        let constant = MirInstr::Const {
            dest: Reg(0),
            k: Const::Int(1),
        };
        let copy = MirInstr::UseVar {
            dest: Reg(1),
            var: 0,
            mode: UseMode::Copy,
        };
        let borrow = MirInstr::UseVar {
            dest: Reg(2),
            var: 0,
            mode: UseMode::BorrowShared,
        };
        assert!(constant.would_be_trivially_dead());
        assert!(borrow.would_be_trivially_dead());
        assert!(!copy.would_be_trivially_dead());
        assert!(!MirInstr::DropVar { var: 0 }.would_be_trivially_dead());
    }

    #[test]
    fn forwarded_operand_folds_only_an_identity_copy() {
        let lane = |width| {
            Ty::new(TyKind::Simd {
                dtype: mojito_types::types::SimdDtype::Known(Dtype::Int32),
                width: mojito_types::types::SimdWidth::Known(width),
            })
        };
        let param = Ty::new(TyKind::Param {
            binder: mojito_types::param_expr::ParamRef::unbound("T"),
            bounds: Vec::new(),
            callable_bound: None,
        });
        let folds = |source: Ty, dest: Ty| {
            let reg_types = HashMap::from([(0, source), (1, dest)]);
            MirInstr::CopyValue {
                dest: Reg(1),
                value: Reg(0),
            }
            .forwarded_operand(&reg_types, &MirDeclarations::default())
        };
        assert_eq!(
            folds(Ty::new(TyKind::Int), Ty::new(TyKind::Int)),
            Some(Reg(0))
        );
        assert_eq!(folds(lane(1), lane(1)), Some(Reg(0)));
        assert_eq!(folds(param.clone(), param), None);
        let string = Ty::new(TyKind::Struct("String".into(), Vec::new()));
        assert_eq!(folds(string.clone(), string), None);
        assert_eq!(folds(lane(4), lane(4)), Some(Reg(0)));
        let literal = Ty::new(TyKind::StringLiteral);
        assert_eq!(folds(literal.clone(), literal), Some(Reg(0)));
        assert_eq!(
            folds(Ty::new(TyKind::IntLiteral), Ty::new(TyKind::Int)),
            None
        );
        let reg_types = HashMap::from([(0, Ty::new(TyKind::Int)), (1, Ty::new(TyKind::Int))]);
        let rebind = MirInstr::Rebind {
            dest: Reg(1),
            value: Reg(0),
        };
        assert_eq!(
            rebind.forwarded_operand(&reg_types, &MirDeclarations::default()),
            None
        );
    }

    #[test]
    fn copy_is_identity_answers_from_a_trivial_register_passable_declaration() {
        use mojito_checked::checked::StructConformance;
        let param = || {
            Ty::new(TyKind::Param {
                binder: mojito_types::param_expr::ParamRef::unbound("T"),
                bounds: Vec::new(),
                callable_bound: None,
            })
        };
        let structure =
            |name: &str, fields: Vec<Ty>, trait_name: &str, conditions| MirStructDeclaration {
                name: name.into(),
                fields: fields
                    .into_iter()
                    .enumerate()
                    .map(|(at, ty)| (format!("f{at}"), ty))
                    .collect(),
                mut_self_methods: HashSet::new(),
                fieldwise_init: true,
                param_decls: Vec::new(),
                explicit_destructors: HashMap::new(),
                explicit_destroy: None,
                conformances: Vec::new(),
                declared: vec![StructConformance {
                    trait_name: trait_name.into(),
                    conditions,
                }],
                associated_types: Vec::new(),
                parameterized_associated_types: Vec::new(),
                associated_values: Vec::new(),
            };
        let struct_ty = |name: &str| Ty::new(TyKind::Struct(name.into(), Vec::new()));
        let unconditional = || vec![GenericConstraint::Bool(true)];
        let lanes = Ty::new(TyKind::Simd {
            dtype: mojito_types::types::SimdDtype::Known(Dtype::Int32),
            width: mojito_types::types::SimdWidth::Known(4),
        });
        let string = struct_ty("String");
        let trivial = "TrivialRegisterPassable";
        let mut declarations = MirDeclarations {
            structs: vec![
                structure(
                    "P",
                    vec![Ty::new(TyKind::Int), lanes.clone()],
                    trivial,
                    unconditional(),
                ),
                structure("Nested", vec![struct_ty("P")], trivial, unconditional()),
                structure(
                    "Copied",
                    vec![Ty::new(TyKind::Int)],
                    trivial,
                    unconditional(),
                ),
                structure("Holds", vec![string], trivial, unconditional()),
                structure("Generic", vec![param()], trivial, unconditional()),
                structure(
                    "Conditional",
                    vec![Ty::new(TyKind::Int)],
                    trivial,
                    vec![GenericConstraint::Conforms {
                        param: mojito_types::param_expr::ParamRef::unbound("T"),
                        trait_name: trivial.into(),
                    }],
                ),
                structure(
                    "Implicit",
                    vec![Ty::new(TyKind::Int)],
                    "ImplicitlyCopyable",
                    unconditional(),
                ),
            ],
            ..MirDeclarations::default()
        };
        declarations.functions.push(MirFunctionDeclaration {
            lowered_name: "Copied.__copyinit__".into(),
            param_names: Vec::new(),
            param_types: Vec::new(),
            defaults: Vec::new(),
            required: Vec::new(),
            variadic: None,
            variadic_convention: None,
            variadic_index: None,
            kw_variadic: None,
            kw_variadic_convention: None,
            kw_variadic_index: None,
            positional_only: None,
            keyword_only: None,
            param_decls: Vec::new(),
            has_receiver: false,
            receiver_convention: None,
            param_conventions: Vec::new(),
            ret_ty: Ty::none(),
            returns_reference: false,
            raises: false,
            error_ty: None,
            ref_params: Vec::new(),
            param_writes: Vec::new(),
            availability: Vec::new(),
            enclosing: None,
            region_binders: Vec::new(),
        });
        let answers = |name: &str| declarations.copy_is_identity(&struct_ty(name));
        assert!(answers("P"));
        assert!(answers("Nested"));
        assert!(declarations.copy_is_identity(&lanes));
        assert!(declarations.copy_is_identity(&Ty::new(TyKind::StringLiteral)));
        for kept in [
            "Copied",
            "Holds",
            "Generic",
            "Conditional",
            "Implicit",
            "Tuple",
        ] {
            assert!(!answers(kept), "{kept}");
        }
        assert!(!declarations.copy_is_identity(&param()));
    }

    #[test]
    fn folded_constant_folds_only_what_the_vm_computes_alike() {
        use mojito_common::literal::IntLiteral;
        use mojito_types::ct::CtLane;
        let none = MirDeclarations::default();
        let ty = |kind| Ty::new(kind);
        let binary = |op, a: Const, b: Const, operand: TyKind, dest: TyKind| {
            let reg_types =
                HashMap::from([(0, ty(operand.clone())), (1, ty(operand)), (2, ty(dest))]);
            MirInstr::BinOp {
                op,
                dest: Reg(2),
                a: Reg(0),
                b: Reg(1),
                resolved: None,
                raises: None,
            }
            .folded_constant(
                |reg| [a.clone(), b.clone()].get(reg.0 as usize).cloned(),
                &reg_types,
                &none,
            )
        };
        let literal = |value: i64| Const::IntLiteral(IntLiteral::from(value));
        assert!(matches!(
            binary(InfixOp::Add, literal(2), literal(3), TyKind::IntLiteral, TyKind::IntLiteral),
            Some(Const::IntLiteral(value)) if value == IntLiteral::from(5i64)
        ));
        assert!(matches!(
            binary(
                InfixOp::Lt,
                literal(1),
                literal(2),
                TyKind::IntLiteral,
                TyKind::Bool
            ),
            Some(Const::Bool(true))
        ));
        assert!(matches!(
            binary(
                InfixOp::Add,
                Const::Int(i64::MAX),
                Const::Int(1),
                TyKind::Int,
                TyKind::Int
            ),
            Some(Const::Int(i64::MIN))
        ));
        for (op, a, b) in [
            (InfixOp::FloorDiv, Const::Int(7), Const::Int(0)),
            (InfixOp::Shl, Const::Int(1), Const::Int(64)),
            (InfixOp::Div, Const::Int(1), Const::Int(2)),
        ] {
            assert!(binary(op, a, b, TyKind::Int, TyKind::Int).is_none());
        }
        assert!(
            binary(
                InfixOp::Add,
                Const::Int(1),
                Const::Int(2),
                TyKind::UInt,
                TyKind::UInt
            )
            .is_none()
        );
        assert!(
            binary(
                InfixOp::BitAnd,
                Const::Bool(true),
                Const::Bool(false),
                TyKind::Bool,
                TyKind::Bool
            )
            .is_none()
        );
        let nan = Const::Float(f64::NAN);
        assert!(binary(InfixOp::Lt, nan.clone(), nan, TyKind::Float64, TyKind::Bool).is_none());
        let param = Const::Param(
            ParamContext::new()
                .constant(CtValue::Int(1))
                .expect("a closed Int is a parameter expression"),
        );
        assert!(binary(InfixOp::Add, param.clone(), param, TyKind::Int, TyKind::Int).is_none());

        let reg_types = HashMap::from([
            (0, ty(TyKind::Int)),
            (1, ty(TyKind::Int)),
            (2, ty(TyKind::Int)),
        ]);
        let ones = |_: Reg| Some(Const::Int(1));
        let raising = MirInstr::BinOp {
            op: InfixOp::Add,
            dest: Reg(2),
            a: Reg(0),
            b: Reg(1),
            resolved: None,
            raises: Some(ty(TyKind::Int)),
        };
        assert!(raising.folded_constant(ones, &reg_types, &none).is_none());
        let dunder = MirInstr::BinOp {
            op: InfixOp::Add,
            dest: Reg(2),
            a: Reg(0),
            b: Reg(1),
            resolved: Some("Int.__add__".into()),
            raises: None,
        };
        assert!(dunder.folded_constant(ones, &reg_types, &none).is_none());

        let materialize = |target: TyKind| {
            let reg_types = HashMap::from([(0, ty(TyKind::IntLiteral)), (1, ty(target.clone()))]);
            MirInstr::MaterializeLiteral {
                dest: Reg(1),
                value: Reg(0),
                target: ty(target),
            }
            .folded_constant(
                |_| {
                    Some(Const::IntLiteral(
                        IntLiteral::from(1i64).shl(&IntLiteral::from(63i64))?,
                    ))
                },
                &reg_types,
                &none,
            )
        };
        assert!(matches!(
            materialize(TyKind::Int),
            Some(Const::Int(i64::MIN))
        ));
        assert!(matches!(
            materialize(TyKind::UInt),
            Some(Const::Value(CtValue::UInt(value))) if value == 1 << 63
        ));

        let lane = |dtype| mojito_types::types::canonical_simd_ty(dtype, 1);
        let materialize_into = |literal: Const, source: TyKind, target: Ty| {
            let reg_types = HashMap::from([(0, ty(source)), (1, target.clone())]);
            MirInstr::MaterializeLiteral {
                dest: Reg(1),
                value: Reg(0),
                target,
            }
            .folded_constant(|_| Some(literal.clone()), &reg_types, &none)
        };
        let simd = |dtype, lane| {
            Const::Value(CtValue::Simd {
                dtype,
                lanes: vec![lane],
            })
        };
        let into_lane =
            |value: i64, dtype| materialize_into(literal(value), TyKind::IntLiteral, lane(dtype));
        assert!(matches!(
            (into_lane(250, Dtype::UInt8), simd(Dtype::UInt8, CtLane::Int(250))),
            (Some(Const::Value(a)), Const::Value(b)) if a == b
        ));
        assert!(matches!(
            (into_lane(300, Dtype::UInt8), simd(Dtype::UInt8, CtLane::Int(44))),
            (Some(Const::Value(a)), Const::Value(b)) if a == b
        ));
        assert!(matches!(
            materialize_into(literal(-1), TyKind::IntLiteral, ty(TyKind::UInt)),
            Some(Const::Value(CtValue::UInt(u64::MAX)))
        ));
        let tenth =
            mojito_common::literal::FloatLiteral::parse_decimal("0.1").expect("decimal literal");
        assert!(matches!(
            materialize_into(
                Const::FloatLiteral(tenth),
                TyKind::FloatLiteral,
                lane(Dtype::Float32)
            ),
            Some(Const::Value(CtValue::Simd { lanes, .. }))
                if lanes == [CtLane::Float(f64::from(0.1f32).to_bits())]
        ));
        assert!(into_lane(1, Dtype::Bool).is_none());
        assert!(into_lane(1, Dtype::Int32).is_some());
        assert!(
            materialize_into(
                literal(1),
                TyKind::IntLiteral,
                mojito_types::types::canonical_simd_ty(Dtype::Int32, 4)
            )
            .is_none()
        );

        let uint = Const::Value(CtValue::UInt(1));
        assert!(matches!(
            binary(
                InfixOp::Add,
                uint.clone(),
                uint.clone(),
                TyKind::UInt,
                TyKind::UInt
            ),
            Some(Const::Value(CtValue::UInt(2)))
        ));
        assert!(
            binary(
                InfixOp::FloorDiv,
                Const::Value(CtValue::UInt(7)),
                Const::Value(CtValue::UInt(0)),
                TyKind::UInt,
                TyKind::UInt
            )
            .is_none()
        );

        let lanes = |op, a: Const, b: Const, operand: Ty, dest: Ty| {
            let right = match &b {
                Const::IntLiteral(_) => ty(TyKind::IntLiteral),
                Const::FloatLiteral(_) => ty(TyKind::FloatLiteral),
                _ => operand.clone(),
            };
            let reg_types = HashMap::from([(0, operand), (1, right), (2, dest)]);
            MirInstr::BinOp {
                op,
                dest: Reg(2),
                a: Reg(0),
                b: Reg(1),
                resolved: None,
                raises: None,
            }
            .folded_constant(
                |reg| [a.clone(), b.clone()].get(reg.0 as usize).cloned(),
                &reg_types,
                &none,
            )
        };
        let byte = |value| simd(Dtype::UInt8, CtLane::Int(value));
        assert!(matches!(
            lanes(InfixOp::Add, byte(250), literal(10), lane(Dtype::UInt8), lane(Dtype::UInt8)),
            Some(Const::Value(CtValue::Simd { lanes, .. })) if lanes == [CtLane::Int(4)]
        ));
        assert!(matches!(
            lanes(InfixOp::FloorDiv, byte(7), byte(0), lane(Dtype::UInt8), lane(Dtype::UInt8)),
            Some(Const::Value(CtValue::Simd { lanes, .. })) if lanes == [CtLane::Int(0)]
        ));
        assert!(matches!(
            lanes(InfixOp::Lt, byte(3), byte(4), lane(Dtype::UInt8), lane(Dtype::Bool)),
            Some(Const::Value(CtValue::Simd { dtype: Dtype::Bool, lanes }))
                if lanes == [CtLane::Bool(true)]
        ));
        assert!(
            lanes(
                InfixOp::Lt,
                byte(3),
                byte(4),
                lane(Dtype::UInt8),
                ty(TyKind::Bool)
            )
            .is_none()
        );
        assert!(
            lanes(
                InfixOp::Shl,
                byte(1),
                byte(9),
                lane(Dtype::UInt8),
                lane(Dtype::UInt8)
            )
            .is_none()
        );
        let single = |value: f64| simd(Dtype::Float32, CtLane::Float(value.to_bits()));
        assert!(matches!(
            lanes(
                InfixOp::Mul,
                single(1.5),
                Const::FloatLiteral(mojito_common::literal::FloatLiteral::from_int(
                    &IntLiteral::from(2i64)
                )),
                lane(Dtype::Float32),
                lane(Dtype::Float32)
            ),
            Some(Const::Value(CtValue::Simd { lanes, .. }))
                if lanes == [CtLane::Float(3.0f64.to_bits())]
        ));
        assert!(
            lanes(
                InfixOp::FloorDiv,
                single(7.5),
                single(2.0),
                lane(Dtype::Float32),
                lane(Dtype::Float32)
            )
            .is_none()
        );

        let make = |width: i64| {
            let target = mojito_types::types::canonical_simd_ty(Dtype::UInt8, width);
            let reg_types = HashMap::from([(0, ty(TyKind::IntLiteral)), (1, target)]);
            MirInstr::MakeSimd {
                dest: Reg(1),
                dtype: mojito_types::types::SimdDtype::Known(Dtype::UInt8),
                width: mojito_types::types::SimdWidth::Known(width),
                elems: vec![Reg(0); width as usize],
            }
            .folded_constant(|_| Some(literal(7)), &reg_types, &none)
        };
        assert!(matches!(
            make(1),
            Some(Const::Value(CtValue::Simd { lanes, .. })) if lanes == [CtLane::Int(7)]
        ));
        assert!(make(4).is_none());
        let reg_types = HashMap::from([(0, ty(TyKind::UInt)), (1, ty(TyKind::UInt))]);
        assert!(matches!(
            MirInstr::CopyValue {
                dest: Reg(1),
                value: Reg(0),
            }
            .folded_constant(|_| Some(uint.clone()), &reg_types, &none),
            Some(Const::Value(CtValue::UInt(1)))
        ));
    }

    #[test]
    fn folded_constant_folds_lane_splats() {
        use mojito_common::literal::{FloatLiteral, IntLiteral};
        use mojito_types::ct::CtLane;
        use mojito_types::types::canonical_simd_ty;
        let none = MirDeclarations::default();
        let literal = |value: i64| Const::IntLiteral(IntLiteral::from(value));
        let byte = |value| {
            Const::Value(CtValue::Simd {
                dtype: Dtype::UInt8,
                lanes: vec![CtLane::Int(value)],
            })
        };
        let splat = |element: Const, source: Ty, dtype, width: i64| {
            let reg_types = HashMap::from([(0, source), (1, canonical_simd_ty(dtype, width))]);
            MirInstr::MakeSimd {
                dest: Reg(1),
                dtype: mojito_types::types::SimdDtype::Known(dtype),
                width: mojito_types::types::SimdWidth::Known(width),
                elems: vec![Reg(0)],
            }
            .folded_constant(|_| Some(element.clone()), &reg_types, &none)
        };
        let lanes_of = |folded: Option<Const>| match folded {
            Some(Const::Value(CtValue::Simd { lanes, .. })) => Some(lanes),
            _ => None,
        };
        let int_literal = Ty::new(TyKind::IntLiteral);
        assert_eq!(
            lanes_of(splat(literal(300), int_literal.clone(), Dtype::UInt8, 4)),
            Some(vec![CtLane::Int(44); 4])
        );
        let tenth = FloatLiteral::parse_decimal("0.1").expect("decimal literal");
        assert_eq!(
            lanes_of(splat(
                Const::FloatLiteral(tenth),
                Ty::new(TyKind::FloatLiteral),
                Dtype::Float32,
                8
            )),
            Some(vec![CtLane::Float(f64::from(0.1f32).to_bits()); 8])
        );
        assert!(splat(literal(1), int_literal, Dtype::Bool, 4).is_none());
        let uint8 = canonical_simd_ty(Dtype::UInt8, 1);
        assert_eq!(
            lanes_of(splat(byte(7), uint8.clone(), Dtype::UInt8, 4)),
            Some(vec![CtLane::Int(7); 4])
        );
        assert!(splat(byte(7), uint8, Dtype::Int8, 4).is_none());
        let bool_ty = Ty::new(TyKind::Bool);
        assert_eq!(
            lanes_of(splat(Const::Bool(true), bool_ty.clone(), Dtype::Bool, 4)),
            Some(vec![CtLane::Bool(true); 4])
        );
        assert!(splat(Const::Bool(true), bool_ty, Dtype::Bool, 1).is_none());

        let quad = canonical_simd_ty(Dtype::Int32, 4);
        let ones = Const::Value(CtValue::Simd {
            dtype: Dtype::Int32,
            lanes: vec![CtLane::Int(1); 4],
        });
        let reg_types = HashMap::from([
            (0, quad.clone()),
            (1, Ty::new(TyKind::IntLiteral)),
            (2, quad),
        ]);
        let sum = MirInstr::BinOp {
            op: InfixOp::Add,
            dest: Reg(2),
            a: Reg(0),
            b: Reg(1),
            resolved: None,
            raises: None,
        }
        .folded_constant(
            |reg| [ones.clone(), literal(2)].get(reg.0 as usize).cloned(),
            &reg_types,
            &none,
        );
        assert_eq!(lanes_of(sum), Some(vec![CtLane::Int(3); 4]));
        assert!(const_ct_value(&ones, &canonical_simd_ty(Dtype::Int32, 2)).is_none());
    }

    #[test]
    fn folded_constant_folds_per_lane_construction() {
        use mojito_common::literal::{FloatLiteral, IntLiteral};
        use mojito_types::ct::CtLane;
        use mojito_types::types::canonical_simd_ty;
        let none = MirDeclarations::default();
        let literal = |value: i64| {
            (
                Const::IntLiteral(IntLiteral::from(value)),
                TyKind::IntLiteral,
            )
        };
        let make = |elements: Vec<(Option<Const>, TyKind)>, dtype, width: i64| {
            let dest = elements.len() as u32;
            let mut reg_types: HashMap<u32, Ty> = (0..dest)
                .zip(elements.iter().map(|(_, kind)| Ty::new(kind.clone())))
                .collect();
            reg_types.insert(dest, canonical_simd_ty(dtype, width));
            let folded = MirInstr::MakeSimd {
                dest: Reg(dest),
                dtype: mojito_types::types::SimdDtype::Known(dtype),
                width: mojito_types::types::SimdWidth::Known(width),
                elems: (0..dest).map(Reg).collect(),
            }
            .folded_constant(
                |reg| elements.get(reg.0 as usize).and_then(|(c, _)| c.clone()),
                &reg_types,
                &none,
            );
            match folded {
                Some(Const::Value(CtValue::Simd { lanes, .. })) => Some(lanes),
                _ => None,
            }
        };
        let some = |constant: Const, kind| (Some(constant), kind);
        let literals = |values: &[i64]| -> Vec<(Option<Const>, TyKind)> {
            values
                .iter()
                .map(|value| {
                    let (constant, kind) = literal(*value);
                    some(constant, kind)
                })
                .collect()
        };
        assert_eq!(
            make(literals(&[1, 2, 3, 4]), Dtype::Int32, 4),
            Some((1..=4).map(CtLane::Int).collect())
        );
        assert_eq!(
            make(literals(&[300, -1, 2, 3]), Dtype::UInt8, 4),
            Some(vec![
                CtLane::Int(44),
                CtLane::Int(255),
                CtLane::Int(2),
                CtLane::Int(3)
            ])
        );
        let tenth = FloatLiteral::parse_decimal("0.1").expect("decimal literal");
        assert_eq!(
            make(
                vec![
                    some(Const::FloatLiteral(tenth), TyKind::FloatLiteral),
                    some(literal(1).0, TyKind::IntLiteral)
                ],
                Dtype::Float32,
                2
            ),
            Some(vec![
                CtLane::Float(f64::from(0.1f32).to_bits()),
                CtLane::Float(1f64.to_bits())
            ])
        );
        assert_eq!(
            make(
                vec![
                    some(Const::Bool(true), TyKind::Bool),
                    some(Const::Bool(false), TyKind::Bool)
                ],
                Dtype::Bool,
                2
            ),
            Some(vec![CtLane::Bool(true), CtLane::Bool(false)])
        );
        let seven = Const::Value(CtValue::Simd {
            dtype: Dtype::Int32,
            lanes: vec![CtLane::Int(7)],
        });
        let int32 = canonical_simd_ty(Dtype::Int32, 1).kind().clone();
        assert_eq!(
            make(
                vec![some(seven, int32), some(literal(2).0, TyKind::IntLiteral)],
                Dtype::Int32,
                2
            ),
            Some(vec![CtLane::Int(7), CtLane::Int(2)])
        );
        assert!(
            make(
                vec![
                    some(Const::Int(7), TyKind::Int),
                    some(literal(2).0, TyKind::IntLiteral)
                ],
                Dtype::Int32,
                2
            )
            .is_none()
        );
        assert!(make(literals(&[1, 2, 3]), Dtype::Int32, 4).is_none());
        let mut unknown = literals(&[1, 2]);
        unknown[1].0 = None;
        assert!(make(unknown, Dtype::Int32, 2).is_none());
    }

    #[test]
    fn folded_constant_folds_scalar_conversions() {
        use mojito_common::literal::IntLiteral;
        let none = MirDeclarations::default();
        let conversion =
            |name: &str, kwargs: Vec<(String, Reg)>, operand: Const, source, target| {
                let reg_types = HashMap::from([(0, Ty::new(source)), (1, Ty::new(target))]);
                MirInstr::Call {
                    dest: Reg(1),
                    func: FuncRef::named(name),
                    raises: None,
                    args: vec![Reg(0)],
                    kwargs,
                    arg_places: vec![None],
                    kwarg_places: Vec::new(),
                    capture_accesses: Vec::new(),
                    param_arg_regs: Vec::new(),
                    receiver: None,
                    instantiated_args: Vec::new(),
                    spread: None,
                }
                .folded_constant(|_| Some(operand.clone()), &reg_types, &none)
            };
        assert!(matches!(
            conversion("UInt", Vec::new(), Const::Int(7), TyKind::Int, TyKind::UInt),
            Some(Const::Value(CtValue::UInt(7)))
        ));
        assert!(matches!(
            conversion(
                "Bool",
                Vec::new(),
                Const::IntLiteral(IntLiteral::from(1)),
                TyKind::IntLiteral,
                TyKind::Bool
            ),
            Some(Const::Bool(true))
        ));
        let huge = Const::Float(1e30);
        assert!(conversion("Int", Vec::new(), huge, TyKind::Float64, TyKind::Int).is_none());
        let keyword = vec![("x".to_string(), Reg(0))];
        assert!(conversion("Int", keyword, Const::Int(7), TyKind::Int, TyKind::Int).is_none());
        assert!(conversion("print", Vec::new(), Const::Int(7), TyKind::Int, TyKind::Int).is_none());
    }
}

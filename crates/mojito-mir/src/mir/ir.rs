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
/// The function level and each `try` region are pruned alike, a region from
/// its own entry. A block a region's `EscapeJump` names is reachable through
/// the block holding the region. Registers, slots, and spans are untouched.
pub fn prune_unreachable_blocks(function: &mut MirFunction) {
    let kept = prune_block_list(&mut function.blocks);
    renumber_escapes(&mut function.blocks, &kept);
}

/// Every register a terminator reads, mutably.
pub fn terminator_regs_mut(term: &mut MirTerm) -> Vec<&mut Reg> {
    match term {
        MirTerm::Branch { cond, .. } => vec![cond],
        MirTerm::Return(Some(reg))
        | MirTerm::ReturnWithCleanup {
            value: Some(reg), ..
        } => vec![reg],
        MirTerm::Jump(_)
        | MirTerm::ComptimeBranch { .. }
        | MirTerm::ComptimeFor { .. }
        | MirTerm::Return(None)
        | MirTerm::ReturnWithCleanup { value: None, .. }
        | MirTerm::FallOff
        | MirTerm::EscapeJump { .. } => Vec::new(),
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
        | MirTerm::FallOff
        | MirTerm::EscapeJump { .. } => Vec::new(),
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
        | MirTerm::FallOff
        | MirTerm::EscapeJump { .. } => Vec::new(),
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
/// none of a `try` region's blocks, which the caller walks itself.
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
        MirInstr::MakeRef {
            dest,
            place: target,
        } => {
            out.push(dest);
            place_regs_mut(target, &mut out);
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
        | MirInstr::DropVar { .. }
        | MirInstr::ConsumeVar { .. }
        | MirInstr::GetIter { .. }
        | MirInstr::Unsupported(_)
        | MirInstr::Try { .. } => {}
        MirInstr::ConstructTypeParam {
            dest,
            kwargs,
            kwarg_places,
            element,
            ..
        } => {
            out.push(dest);
            keyword_regs_mut(kwargs, kwarg_places, &mut out);
            out.extend(
                element
                    .iter_mut()
                    .filter_map(|argument| argument.value.as_mut()),
            );
        }
        MirInstr::Const { dest, .. }
        | MirInstr::SizeOf { dest, .. }
        | MirInstr::TypeName { dest, .. }
        | MirInstr::HasNext { dest, .. }
        | MirInstr::Next { dest, .. } => out.push(dest),
        MirInstr::MaterializeLiteral { dest, value, .. } => out.extend([dest, value]),
        MirInstr::UseVar { dest, .. } => out.push(dest),
        MirInstr::MovePlace {
            dest,
            place: target,
        }
        | MirInstr::LoadPlace {
            dest,
            place: target,
        } => {
            out.push(dest);
            place_regs_mut(target, &mut out);
        }
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
            places_regs_mut(callee_place.iter_mut(), &mut out);
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
            places_regs_mut(recv_place.iter_mut(), &mut out);
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
            places_regs_mut(base_place.iter_mut(), &mut out);
            places_regs_mut(index_place.iter_mut(), &mut out);
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
            places_regs_mut(object_place.iter_mut(), &mut out);
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
            places_regs_mut(object_place.iter_mut(), &mut out);
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
            places_regs_mut(receiver_place.iter_mut(), &mut out);
            places_regs_mut(arg_places.iter_mut().flatten(), &mut out);
            places_regs_mut(value_place.iter_mut(), &mut out);
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
        match &mut block.term {
            MirTerm::Jump(to) => target(to),
            MirTerm::Branch { then_b, else_b, .. }
            | MirTerm::ComptimeBranch { then_b, else_b, .. }
            | MirTerm::ComptimeFor {
                body: then_b,
                exit: else_b,
                ..
            } => {
                target(then_b);
                target(else_b);
            }
            MirTerm::Return(_)
            | MirTerm::ReturnWithCleanup { .. }
            | MirTerm::FallOff
            | MirTerm::EscapeJump { .. } => {}
        }
        for region in block.instrs.iter_mut().flat_map(try_regions_mut) {
            prune_block_list(region);
        }
    }
    kept
}

/// The blocks of the same list a block hands control to: its terminator's
/// targets, and the escape targets of every region it holds.
pub fn block_successors(block: &MirBlock) -> Vec<MirBlockId> {
    let mut successors = match &block.term {
        MirTerm::Jump(to) => vec![*to],
        MirTerm::Branch { then_b, else_b, .. }
        | MirTerm::ComptimeBranch { then_b, else_b, .. }
        | MirTerm::ComptimeFor {
            body: then_b,
            exit: else_b,
            ..
        } => {
            vec![*then_b, *else_b]
        }
        MirTerm::Return(_)
        | MirTerm::ReturnWithCleanup { .. }
        | MirTerm::FallOff
        | MirTerm::EscapeJump { .. } => Vec::new(),
    };
    for region in block.instrs.iter().flat_map(try_regions) {
        collect_escape_targets(region, &mut successors);
    }
    successors
}

fn collect_escape_targets(blocks: &[MirBlock], targets: &mut Vec<MirBlockId>) {
    for block in blocks {
        if let MirTerm::EscapeJump { target, .. } = &block.term {
            targets.push(*target);
        }
        for region in block.instrs.iter().flat_map(try_regions) {
            collect_escape_targets(region, targets);
        }
    }
}

/// Renumber every escape target below `blocks` with the function-level table.
fn renumber_escapes(blocks: &mut [MirBlock], kept: &[Option<usize>]) {
    for block in blocks {
        for region in block.instrs.iter_mut().flat_map(try_regions_mut) {
            for inner in region.iter_mut() {
                if let MirTerm::EscapeJump { target, .. } = &mut inner.term {
                    *target = kept[*target].expect("an escape names a surviving block");
                }
            }
            renumber_escapes(region, kept);
        }
    }
}

fn try_regions(instruction: &MirInstr) -> Vec<&Vec<MirBlock>> {
    match instruction {
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

fn try_regions_mut(instruction: &mut MirInstr) -> Vec<&mut Vec<MirBlock>> {
    match instruction {
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

/// Whether a `try` region's statements contain a `break`/`continue` that **leaves**
/// the region — targeting a loop *outside* it. Such an escape would need to name the
/// outer loop's target block, which the self-contained mini-CFG region can't express
/// (unlike a `return`, which surfaces as a `Flow::Return` the block driver handles).
/// Nested loops absorb their own `break`/`continue` (tracked via `loop_depth`);
/// nested `def`/`struct` bodies have their own control flow and are not scanned.
pub(super) fn region_crosses_control(body: &[Stmt]) -> bool {
    fn walk(stmts: &[Stmt], loop_depth: usize) -> bool {
        stmts.iter().any(|s| match &s.kind {
            StmtKind::Break | StmtKind::Continue => loop_depth == 0,
            StmtKind::If { branches, orelse } => {
                branches.iter().any(|(_, b)| walk(b, loop_depth))
                    || orelse.as_ref().is_some_and(|b| walk(b, loop_depth))
            }
            StmtKind::While { body, .. } | StmtKind::For { body, .. } => walk(body, loop_depth + 1),
            StmtKind::Scope(body) => walk(body, loop_depth),
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                walk(body, loop_depth)
                    || except.as_ref().is_some_and(|(_, b)| walk(b, loop_depth))
                    || orelse.as_ref().is_some_and(|b| walk(b, loop_depth))
                    || finalbody.as_ref().is_some_and(|b| walk(b, loop_depth))
            }
            _ => false,
        })
    }
    walk(body, 0)
}

/// Whether an argument convention is a written-back reference (`mut`/`ref`).
#[allow(
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    reason = "TODO: take Option<&T>; TODO: take by value and update the call sites"
)]
pub(super) const fn is_ref(c: &Option<ArgConvention>) -> bool {
    matches!(c, Some(ArgConvention::Mut | ArgConvention::Ref))
}
use mojito_hir::hir::VarId;
use std::collections::HashMap;

/// A virtual ("infinite") register — a fresh one per intermediate value (SSA-ish).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirParamArg {
    pub name: Option<String>,
    pub value: Option<Reg>,
    /// The enclosing declaration's type binder this argument forwards
    /// (`hash[Self.H](key)`), whose spelling `value` reifies.
    pub binder: Option<mojito_types::param_expr::ParamRef>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseMode {
    Copy,
    Move,
    BorrowShared,
    BorrowMut,
}

/// One already-evaluated argument to a multi-dimensional subscript.
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone)]
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
    /// A closed vector- or struct-typed parameter value read as a runtime
    /// value (`Self.key` of an `AHasher[key: U256]` instance): upstream's
    /// `kgen.param.constant` over a constant attribute. The elaborator folds
    /// a binder read to it; it holds only a `CtValue::Simd` or a
    /// `CtValue::Struct` of such values and scalars.
    Value(mojito_types::ct::CtValue),
}

/// The callee of a `MirInstr::Call` — a function/struct-constructor/builtin name.
/// (Resolution to a concrete target happens in the backend's assembler.)
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
}

/// A writable location: a root variable plus a chain of projections
/// (`p.items[i].x` = root `p`, proj `[Field("items"), Index(i), Field("x")]`).
#[derive(Debug, Clone)]
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
}

/// Canonical, runtime-erased identity of an interior storage generation after
/// stable checker owners have been mapped to MIR slots.
///
/// `Interior` path segments are invalidation domains; ordinary field/index
/// segments retain field sensitivity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
/// loans it is an ordinary borrow.
#[derive(Debug, Clone)]
pub struct MirLoan {
    pub place: MirPlace,
    pub mutable: bool,
    pub interior: Option<MirInteriorOrigin>,
    pub shared: bool,
}

/// Complete checker-selected contract for a nominal subscript invocation.
/// Intrinsic pointer/SIMD/private-storage operations carry no such payload.
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
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
    /// The byte size of one checker-resolved type: a layout query only a
    /// generator carries. The elaborator answers it under the compilation's
    /// native target, so elaborated MIR holds the constant and never the
    /// query.
    SizeOf {
        dest: Reg,
        ty: Ty,
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
    /// `UseVar { mode: Move }`; user-facing indexed transfers remain restricted
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
    DefVar {
        var: VarId,
        src: Reg,
        binding_ty: Option<Ty>,
    },
    UnOp {
        op: PrefixOp,
        dest: Reg,
        a: Reg,
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
        callee_place: Option<MirPlace>,
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
        reference_result: Option<mojito_types::origin::RefTy>,
        /// Checker-proven adaptation from a concrete implementation ABI to the
        /// abstract result promised at this call site.
        result_adapter: Option<mojito_checked::checked::CheckedResultAdapter>,
        args: Vec<Reg>,
        kwargs: Vec<(String, Reg)>,
        recv_place: Option<MirPlace>,
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
        base_place: Option<MirPlace>,
        index_place: Option<MirPlace>,
        call: Option<MirSubscriptCall>,
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
        object_place: Option<MirPlace>,
        arg_places: Vec<Option<MirPlace>>,
        call: Option<MirSubscriptCall>,
        intrinsic: Option<MirIntrinsicSubscript>,
    },
    /// `object[a, b:c]`: variadic `__getitem__` dispatch with every slice
    /// descriptor selected by the checker and constructed explicitly by the VM.
    MultiIndex {
        dest: Reg,
        object: Reg,
        args: Vec<MirSubscriptArg>,
        object_place: Option<MirPlace>,
        arg_places: Vec<Option<MirPlace>>,
        /// Keyword subscript actuals — index values (`s[byte=i]`) or slice
        /// descriptors (`s[byte=a:b]`) — bound to keyword-only `__getitem__`
        /// parameters through the checked contract's keyword sources,
        /// parallel to `kwarg_places`.
        kwargs: Vec<(String, MirSubscriptArg)>,
        kwarg_places: Vec<Option<MirPlace>>,
        call: Option<MirSubscriptCall>,
    },
    /// `object[a, b:c] = value`: checked `__setitem__` dispatch. The receiver
    /// place is retained so a `mut self` implementation is written back after
    /// the call. Variadic setitem methods receive `value` in their keyword-only
    /// slot; fixed-arity methods receive it as the last positional argument.
    MultiSet {
        receiver: Reg,
        receiver_place: Option<MirPlace>,
        args: Vec<MirSubscriptArg>,
        arg_places: Vec<Option<MirPlace>>,
        value: Reg,
        value_place: Option<MirPlace>,
        value_keyword: bool,
        call: MirSubscriptCall,
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
        call: mojito_checked::checked::CheckedIteratorCall,
        exhaustion: Ty,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirCaptureMode {
    Reference,
    Copy,
    Move,
}

#[derive(Debug, Clone)]
pub struct MirClosureCapture {
    pub place: MirPlace,
    pub mode: MirCaptureMode,
}

/// A static access to owner storage performed by a callable environment while
/// executing a call. Origin paths deliberately retain abstract indices; the VM
/// never interprets this metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirCaptureAccess {
    pub root: VarId,
    pub path: Vec<mojito_types::origin::OriginSeg>,
    pub access: mojito_types::origin::CaptureAccess,
}

/// How a basic block hands off control. Block targets are indices into
/// `MirFunction::blocks`; values are registers.
#[derive(Debug, Clone)]
pub enum MirTerm {
    Jump(MirBlockId),
    Branch {
        cond: Reg,
        then_b: MirBlockId,
        else_b: MirBlockId,
    },
    /// A `comptime if`: a branch whose condition is a parameter expression
    /// over the function's binders, decided by the elaborator, which keeps
    /// the taken arm. Ownership and drops treat it as `Branch` with the
    /// condition opaque; concrete MIR carries none.
    ComptimeBranch {
        cond: Box<GenericConstraint>,
        then_b: MirBlockId,
        else_b: MirBlockId,
    },
    /// A `comptime for` header: a loop whose variable is the parameter
    /// binder `binder`, read by the body through the slot `slot`, over the
    /// compile-time sequence `source` — a `range` of parameter expressions,
    /// or a parameter expression whose value yields the elements. The body's
    /// back edge jumps here; `exit` follows the loop. Ownership and drops
    /// treat it as a loop with its trip count unknown; the elaborator unrolls
    /// it, so concrete MIR carries none.
    ComptimeFor {
        binder: mojito_types::param_expr::ParamRef,
        slot: VarId,
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
    /// Normal fall-through end of a `try` sub-region (see [`hir::Terminator::FallOff`]).
    /// The VM's region runner reads it as "completed normally". Never appears in a
    /// function body's blocks.
    FallOff,
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
}

#[derive(Debug, Clone)]
pub struct MirBlock {
    pub instrs: Vec<MirInstr>,
    pub term: MirTerm,
}

#[derive(Debug, Clone)]
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
    pub var_tys: HashMap<VarId, Ty>,
    /// Checked return type. `None` only on unchecked compatibility paths;
    /// production lowering always records it (`Ty::None` for no return).
    pub ret_ty: Option<Ty>,
    /// Checked raising contract (`raises Never` records as nonraising).
    pub raises: bool,
    /// Declared error type when `raises` is true and the contract is typed.
    pub error_ty: Option<Ty>,
    pub spans: SpanTable,
    /// Resolved type of registers originating in checked expressions. Synthetic
    /// control-flow registers are filled by instruction typing before verification.
    pub reg_types: HashMap<u32, Ty>,
}

/// Maps each generated register to its source span and (if it names one) the
/// origin variable — so borrow-checker diagnostics can point at real code.
#[derive(Debug, Clone, Default)]
pub struct SpanTable(pub HashMap<u32 /*reg*/, (SourceSpan, Option<VarId>)>);

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
}

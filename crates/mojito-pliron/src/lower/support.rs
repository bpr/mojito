//! Free helpers: thunk/pow bodies, C-ABI types, MIR scans, scalar
//! predicates, and instruction metadata.

use super::*;

/// The wrapping square-and-multiply body of `mjrt_pow`. The exponent arrives
/// range-guarded; overflow of the accumulating multiplications wraps, in the
/// same recorded-divergence class as the plain `+`/`-`/`*` operators (the VM's
/// `i64::pow` has no defined overflow semantics).
/// Emit the body of an `invoke` thunk (see [`ModuleShared::ensure_thunk`]):
/// load/take the capture arguments out of the environment record, forward
/// the out-pointer and every user argument unchanged, call the lifted
/// target directly, and return its result.
pub(crate) fn emit_thunk_body(
    ctx: &mut Context,
    func: FuncOp,
    target: &FnSignature,
    modes: &str,
    capture_offsets: &[u64],
    has_out: bool,
) {
    let entry = func.get_or_create_entry_block(ctx);
    let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
    let i8_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
    let mut next_argument = 0;
    let argument = |ctx: &Context, next: &mut usize| {
        let value = entry.deref(ctx).get_argument(*next);
        *next += 1;
        value
    };
    let mut operands = Vec::new();
    if has_out {
        operands.push(argument(ctx, &mut next_argument));
    }
    let env = argument(ctx, &mut next_argument);
    for (mode, offset) in modes.chars().zip(capture_offsets) {
        let slot = if *offset == 0 {
            env
        } else {
            let index = u32::try_from(*offset).expect("environment offsets fit u32");
            let gep = GetElementPtrOp::new(ctx, env, vec![GepIndex::Constant(index)], i8_ty);
            gep.get_operation().insert_at_back(entry, ctx);
            gep.get_result(ctx)
        };
        // Every capture parameter is a reference parameter (the lifted
        // environment prefix): a `Reference` slot stores the captured
        // place's address, an owned (`c`/`m`) slot stores the value inline
        // and passes its own address — the record is the stable storage the
        // VM's owned-capture re-referencing requires.
        if mode == 'r' {
            let load = LoadOp::new(ctx, slot, ptr_ty);
            load.get_operation().insert_at_back(entry, ctx);
            operands.push(load.get_result(ctx));
        } else {
            operands.push(slot);
        }
    }
    let physical_captures = modes.len();
    let mut remaining = target
        .params
        .iter()
        .enumerate()
        .skip(physical_captures)
        .filter(|(index, param)| {
            !matches!(param, LowerTy::ZeroSized)
                || target.ref_params.get(*index).copied().unwrap_or(false)
        })
        .count();
    while remaining > 0 {
        operands.push(argument(ctx, &mut next_argument));
        remaining -= 1;
    }
    let callee: Identifier = target
        .mangled
        .as_str()
        .try_into()
        .expect("mangled names are identifier-safe");
    let call = CallOp::new(
        ctx,
        CallOpCallable::Direct(callee),
        target.func_ty,
        operands,
    );
    call.get_operation().insert_at_back(entry, ctx);
    let result = target.returns_value.then(|| call.get_result(ctx));
    let ret = ReturnOp::new(ctx, result);
    ret.get_operation().insert_at_back(entry, ctx);
}

pub(crate) fn emit_pow_body(ctx: &mut Context, func: FuncOp) {
    let entry = func.get_or_create_entry_block(ctx);
    let region = func
        .get_operation()
        .deref(ctx)
        .regions()
        .next()
        .expect("llvm.func has a body region");
    let i64_ty: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();

    let head = BasicBlock::new(ctx, None, vec![]);
    head.insert_at_back(region, ctx);
    let body = BasicBlock::new(ctx, None, vec![]);
    body.insert_at_back(region, ctx);
    let multiply = BasicBlock::new(ctx, None, vec![]);
    multiply.insert_at_back(region, ctx);
    let advance = BasicBlock::new(ctx, None, vec![]);
    advance.insert_at_back(region, ctx);
    let done = BasicBlock::new(ctx, None, vec![]);
    done.insert_at_back(region, ctx);

    let i64_int = IntegerType::get(ctx, 64, Signedness::Signless);
    pub(super) fn constant(
        ctx: &mut Context,
        i64_int: TypedHandle<IntegerType>,
        value: u64,
        block: Ptr<BasicBlock>,
    ) -> Value {
        let attr = IntegerAttr::new(i64_int, APInt::from_u64(value, bw(64)));
        let op = ConstantOp::new(ctx, Box::new(attr));
        op.get_operation().insert_at_back(block, ctx);
        op.get_result(ctx)
    }

    // entry: allocas for acc/base/exp, seeded from the arguments.
    let one = constant(ctx, i64_int, 1, entry);
    let acc_alloca = AllocaOp::new(ctx, i64_ty, one);
    acc_alloca.get_operation().insert_at_back(entry, ctx);
    let base_alloca = AllocaOp::new(ctx, i64_ty, one);
    base_alloca.get_operation().insert_at_back(entry, ctx);
    let exp_alloca = AllocaOp::new(ctx, i64_ty, one);
    exp_alloca.get_operation().insert_at_back(entry, ctx);
    let acc_slot = acc_alloca.get_result(ctx);
    let base_slot = base_alloca.get_result(ctx);
    let exp_slot = exp_alloca.get_result(ctx);
    let seed_acc = StoreOp::new(ctx, one, acc_slot);
    seed_acc.get_operation().insert_at_back(entry, ctx);
    let base_arg = entry.deref(ctx).get_argument(0);
    let exp_arg = entry.deref(ctx).get_argument(1);
    let seed_base = StoreOp::new(ctx, base_arg, base_slot);
    seed_base.get_operation().insert_at_back(entry, ctx);
    let seed_exp = StoreOp::new(ctx, exp_arg, exp_slot);
    seed_exp.get_operation().insert_at_back(entry, ctx);
    let to_head = BrOp::new(ctx, head, vec![]);
    to_head.get_operation().insert_at_back(entry, ctx);

    // head: while exp != 0.
    let exp0 = LoadOp::new(ctx, exp_slot, i64_ty);
    exp0.get_operation().insert_at_back(head, ctx);
    let zero = constant(ctx, i64_int, 0, head);
    let live = ICmpOp::new(ctx, ICmpPredicateAttr::NE, exp0.get_result(ctx), zero);
    live.get_operation().insert_at_back(head, ctx);
    let head_branch = CondBrOp::new(ctx, live.get_result(ctx), body, vec![], done, vec![]);
    head_branch.get_operation().insert_at_back(head, ctx);

    // body: multiply into acc when the low exponent bit is set.
    let exp1 = LoadOp::new(ctx, exp_slot, i64_ty);
    exp1.get_operation().insert_at_back(body, ctx);
    let bit_one = constant(ctx, i64_int, 1, body);
    let bit = AndOp::new(ctx, exp1.get_result(ctx), bit_one);
    bit.get_operation().insert_at_back(body, ctx);
    let body_zero = constant(ctx, i64_int, 0, body);
    let odd = ICmpOp::new(ctx, ICmpPredicateAttr::NE, bit.get_result(ctx), body_zero);
    odd.get_operation().insert_at_back(body, ctx);
    let body_branch = CondBrOp::new(ctx, odd.get_result(ctx), multiply, vec![], advance, vec![]);
    body_branch.get_operation().insert_at_back(body, ctx);

    // multiply: acc = acc * base (wrapping).
    let acc0 = LoadOp::new(ctx, acc_slot, i64_ty);
    acc0.get_operation().insert_at_back(multiply, ctx);
    let base0 = LoadOp::new(ctx, base_slot, i64_ty);
    base0.get_operation().insert_at_back(multiply, ctx);
    let acc1 = MulOp::new_with_overflow_flag(
        ctx,
        acc0.get_result(ctx),
        base0.get_result(ctx),
        no_overflow_flags(),
    );
    acc1.get_operation().insert_at_back(multiply, ctx);
    let save_acc = StoreOp::new(ctx, acc1.get_result(ctx), acc_slot);
    save_acc.get_operation().insert_at_back(multiply, ctx);
    let multiply_join = BrOp::new(ctx, advance, vec![]);
    multiply_join.get_operation().insert_at_back(multiply, ctx);

    // advance: base = base * base (wrapping); exp >>= 1 (logical).
    let base1 = LoadOp::new(ctx, base_slot, i64_ty);
    base1.get_operation().insert_at_back(advance, ctx);
    let base2 = MulOp::new_with_overflow_flag(
        ctx,
        base1.get_result(ctx),
        base1.get_result(ctx),
        no_overflow_flags(),
    );
    base2.get_operation().insert_at_back(advance, ctx);
    let save_base = StoreOp::new(ctx, base2.get_result(ctx), base_slot);
    save_base.get_operation().insert_at_back(advance, ctx);
    let exp2 = LoadOp::new(ctx, exp_slot, i64_ty);
    exp2.get_operation().insert_at_back(advance, ctx);
    let shift_one = constant(ctx, i64_int, 1, advance);
    let exp3 = LShrOp::new(ctx, exp2.get_result(ctx), shift_one);
    exp3.get_operation().insert_at_back(advance, ctx);
    let save_exp = StoreOp::new(ctx, exp3.get_result(ctx), exp_slot);
    save_exp.get_operation().insert_at_back(advance, ctx);
    let advance_loop = BrOp::new(ctx, head, vec![]);
    advance_loop.get_operation().insert_at_back(advance, ctx);

    // done: return acc.
    let result = LoadOp::new(ctx, acc_slot, i64_ty);
    result.get_operation().insert_at_back(done, ctx);
    let ret = ReturnOp::new(ctx, Some(result.get_result(ctx)));
    ret.get_operation().insert_at_back(done, ctx);
}

/// The LLVM-dialect type of one runtime-contract primitive. LLVM integers
/// are signless, so `U64` and `I64` share `i64`; pointers are opaque.
pub(crate) fn c_abi_type(
    ctx: &mut Context,
    ty: mojito_native::native::rt_abi::CAbiTy,
) -> TypeHandle {
    use mojito_native::native::rt_abi::CAbiTy;
    match ty {
        CAbiTy::U32 => IntegerType::get(ctx, 32, Signedness::Signless).into(),
        CAbiTy::U64 | CAbiTy::I64 => IntegerType::get(ctx, 64, Signedness::Signless).into(),
        CAbiTy::F64 => FP64Type::get(ctx).into(),
        CAbiTy::PtrConstU8 | CAbiTy::PtrMutU8 => PointerType::get(ctx, 0).into(),
    }
}

/// Collect every `MovePlace` with a projection under `blocks`, recursing
/// into `try` sub-regions — the pre-scan that decides which variables need
/// per-leaf presence flags in the entry block.
pub(crate) fn collect_projected_move_places<'m>(
    blocks: &'m [mojito_mir::mir::MirBlock],
    out: &mut Vec<&'m MirPlace>,
) {
    for block in blocks {
        for instr in &block.instrs {
            match instr {
                MirInstr::MovePlace { place, .. } if !place.proj.is_empty() => out.push(place),
                MirInstr::MethodCall {
                    recv_place: Some(place),
                    ..
                } if !place.proj.is_empty() => out.push(place),
                MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } => {
                    collect_projected_move_places(body, out);
                    if let Some((_, handler_blocks)) = handler {
                        collect_projected_move_places(handler_blocks, out);
                    }
                    if let Some(orelse_blocks) = orelse {
                        collect_projected_move_places(orelse_blocks, out);
                    }
                    if let Some(final_blocks) = finalbody {
                        collect_projected_move_places(final_blocks, out);
                    }
                }
                _ => {}
            }
        }
    }
}

pub(crate) fn collect_aliased_receiver_regs(
    function: &MirFunction,
    declarations: &HashMap<String, MirFunctionDeclaration>,
) -> HashSet<u32> {
    pub(super) fn visit(
        function: &MirFunction,
        blocks: &[MirBlock],
        declarations: &HashMap<String, MirFunctionDeclaration>,
        output: &mut HashSet<u32>,
    ) {
        for block in blocks {
            for instruction in &block.instrs {
                match instruction {
                    MirInstr::Call {
                        func,
                        args,
                        kwargs,
                        arg_places,
                        kwarg_places,
                        ..
                    } => {
                        if let Some(declaration) = declarations.get(&func.0) {
                            for (index, (reg, _place)) in args.iter().zip(arg_places).enumerate() {
                                let borrowed = !matches!(
                                    declaration.param_conventions.get(index).copied().flatten(),
                                    Some(
                                        mojito_ast::ast::ArgConvention::Var
                                            | mojito_ast::ast::ArgConvention::Deinit
                                    )
                                );
                                let aggregate =
                                    function.reg_types.get(&reg.0).is_some_and(is_aggregate_ty);
                                if borrowed && aggregate {
                                    output.insert(reg.0);
                                }
                            }
                            for ((name, reg), _place) in kwargs.iter().zip(kwarg_places) {
                                let Some(index) = declaration
                                    .param_names
                                    .iter()
                                    .position(|parameter| parameter == name)
                                else {
                                    continue;
                                };
                                let borrowed = !matches!(
                                    declaration.param_conventions.get(index).copied().flatten(),
                                    Some(
                                        mojito_ast::ast::ArgConvention::Var
                                            | mojito_ast::ast::ArgConvention::Deinit
                                    )
                                );
                                let aggregate =
                                    function.reg_types.get(&reg.0).is_some_and(is_aggregate_ty);
                                if borrowed && aggregate {
                                    output.insert(reg.0);
                                }
                            }
                        }
                        // `Type(copy=place)` is the explicit copy-constructor
                        // boundary. Its borrowed source must stay a place;
                        // cloning the scaffolding LoadPlace would run the
                        // user copy constructor once before the constructor
                        // runs it again.
                        for ((name, reg), place) in kwargs.iter().zip(kwarg_places) {
                            if name == "copy" && place.is_some() {
                                output.insert(reg.0);
                            }
                        }
                    }
                    MirInstr::MethodCall {
                        recv,
                        resolved: Some(resolved),
                        recv_place: Some(_),
                        ..
                    } if declarations.get(resolved).is_some_and(|declaration| {
                        !matches!(
                            declaration.receiver_convention,
                            Some(mojito_ast::ast::ArgConvention::Var)
                        )
                    }) =>
                    {
                        output.insert(recv.0);
                    }
                    MirInstr::Try {
                        body,
                        handler,
                        orelse,
                        finalbody,
                        ..
                    } => {
                        visit(function, body, declarations, output);
                        if let Some((_, blocks)) = handler {
                            visit(function, blocks, declarations, output);
                        }
                        if let Some(blocks) = orelse {
                            visit(function, blocks, declarations, output);
                        }
                        if let Some(blocks) = finalbody {
                            visit(function, blocks, declarations, output);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    let mut output = HashSet::new();
    visit(function, &function.blocks, declarations, &mut output);
    let loaded = collect_loaded_places(&function.blocks);
    output.retain(|reg| loaded.contains_key(reg));
    output
}

pub(crate) fn collect_loaded_places(blocks: &[MirBlock]) -> HashMap<u32, MirPlace> {
    pub(super) fn visit(blocks: &[MirBlock], output: &mut HashMap<u32, MirPlace>) {
        for block in blocks {
            for instruction in &block.instrs {
                match instruction {
                    MirInstr::LoadPlace { dest, place } => {
                        output.insert(dest.0, place.clone());
                    }
                    MirInstr::Try {
                        body,
                        handler,
                        orelse,
                        finalbody,
                        ..
                    } => {
                        visit(body, output);
                        if let Some((_, blocks)) = handler {
                            visit(blocks, output);
                        }
                        if let Some(blocks) = orelse {
                            visit(blocks, output);
                        }
                        if let Some(blocks) = finalbody {
                            visit(blocks, output);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let mut output = HashMap::new();
    visit(blocks, &mut output);
    output
}

/// The SSA scalar shape of a built-in copyable receiver whose `copy()` is the
/// value read (the scalar arms of `lower_ty`); references and pointers are
/// not value copies and aggregates copy through their own lifecycle.
pub(crate) fn scalar_copy_ty(ty: &Ty) -> Option<ScalarTy> {
    match ty {
        Ty::Int | Ty::IntLiteral => Some(ScalarTy::Int),
        Ty::UInt => Some(ScalarTy::UInt),
        Ty::Float64 | Ty::FloatLiteral => Some(ScalarTy::Float64),
        Ty::Bool => Some(ScalarTy::Bool),
        Ty::Simd { dtype, width: 1 } => Some(ScalarTy::of_dtype(*dtype)),
        _ => None,
    }
}

pub(crate) fn is_aggregate_ty(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Struct(..) | Ty::Tuple(..) | Ty::RuntimePack(..) | Ty::Variant(..) | Ty::Func { .. }
    )
}

/// Map a checked type to its scalar lowering, or reject it.
pub(crate) fn scalar_type(
    function: &str,
    ty: &Ty,
    location: Option<SourceSpan>,
) -> Result<ScalarTy, PlironError> {
    match ty {
        Ty::Int => Ok(ScalarTy::Int),
        Ty::UInt => Ok(ScalarTy::UInt),
        Ty::Float64 => Ok(ScalarTy::Float64),
        Ty::Bool => Ok(ScalarTy::Bool),
        Ty::Simd { dtype, width: 1 } => Ok(ScalarTy::of_dtype(*dtype)),
        Ty::Pointer { .. } | Ty::Ref(_) => Ok(ScalarTy::Ptr),
        other => Err(PlironError {
            function: Some(function.to_string()),
            kind: PlironErrorKind::Unsupported {
                construct: format!("type `{other:?}`"),
            },
            location,
        }),
    }
}

pub(crate) fn is_comparison(op: InfixOp) -> bool {
    matches!(
        op,
        InfixOp::Eq | InfixOp::Ne | InfixOp::Lt | InfixOp::Le | InfixOp::Gt | InfixOp::Ge
    )
}

pub(crate) fn signed_predicate(op: InfixOp) -> ICmpPredicateAttr {
    match op {
        InfixOp::Eq => ICmpPredicateAttr::EQ,
        InfixOp::Ne => ICmpPredicateAttr::NE,
        InfixOp::Lt => ICmpPredicateAttr::SLT,
        InfixOp::Le => ICmpPredicateAttr::SLE,
        InfixOp::Gt => ICmpPredicateAttr::SGT,
        InfixOp::Ge => ICmpPredicateAttr::SGE,
        other => unreachable!("`{other:?}` is not a comparison"),
    }
}

pub(crate) fn unsigned_predicate(op: InfixOp) -> ICmpPredicateAttr {
    match op {
        InfixOp::Eq => ICmpPredicateAttr::EQ,
        InfixOp::Ne => ICmpPredicateAttr::NE,
        InfixOp::Lt => ICmpPredicateAttr::ULT,
        InfixOp::Le => ICmpPredicateAttr::ULE,
        InfixOp::Gt => ICmpPredicateAttr::UGT,
        InfixOp::Ge => ICmpPredicateAttr::UGE,
        other => unreachable!("`{other:?}` is not a comparison"),
    }
}

pub(crate) fn float_predicate(op: InfixOp) -> FCmpPredicateAttr {
    match op {
        InfixOp::Eq => FCmpPredicateAttr::OEQ,
        // Rust `!=` on f64 is true for NaN operands: unordered-or-unequal.
        InfixOp::Ne => FCmpPredicateAttr::UNE,
        InfixOp::Lt => FCmpPredicateAttr::OLT,
        InfixOp::Le => FCmpPredicateAttr::OLE,
        InfixOp::Gt => FCmpPredicateAttr::OGT,
        InfixOp::Ge => FCmpPredicateAttr::OGE,
        other => unreachable!("`{other:?}` is not a comparison"),
    }
}

pub(crate) fn no_overflow_flags() -> IntegerOverflowFlagsAttr {
    IntegerOverflowFlagsAttr {
        nsw: false,
        nuw: false,
    }
}

/// Every register `instr` reads (operands, not destinations), for last-use
/// bookkeeping. Instructions outside the supported subset reject before any
/// owned temporary could reach them, so their operands may be approximate.
/// Record final operand appearances over `blocks` (whose position-space ids
/// are `ids`), recursing into `try` regions. Each region's blocks take the
/// next contiguous ids at the moment its `try` is reached, regions in
/// body → handler → orelse → finalbody order — mirroring `lower_region`'s
/// assignment exactly so positions agree.
pub(crate) fn record_last_uses(
    last_uses: &mut HashMap<u32, (usize, usize)>,
    blocks: &[MirBlock],
    ids: &[usize],
    next_id: &mut usize,
) {
    pub(super) fn record_region(
        last_uses: &mut HashMap<u32, (usize, usize)>,
        blocks: &[MirBlock],
        next_id: &mut usize,
    ) {
        let ids: Vec<usize> = (*next_id..*next_id + blocks.len()).collect();
        *next_id += blocks.len();
        record_last_uses(last_uses, blocks, &ids, next_id);
    }
    for (i, block) in blocks.iter().enumerate() {
        for (index, instr) in block.instrs.iter().enumerate() {
            for reg in operand_regs(instr) {
                last_uses.insert(reg.0, (ids[i], index));
            }
            if let MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } = instr
            {
                record_region(last_uses, body, next_id);
                if let Some((_, handler_blocks)) = handler {
                    record_region(last_uses, handler_blocks, next_id);
                }
                if let Some(orelse_blocks) = orelse {
                    record_region(last_uses, orelse_blocks, next_id);
                }
                if let Some(final_blocks) = finalbody {
                    record_region(last_uses, final_blocks, next_id);
                }
            }
        }
        for reg in terminator_regs(&block.term) {
            last_uses.insert(reg.0, (ids[i], usize::MAX));
        }
    }
}

/// One already-lowered subscript actual: an index register (with its checked
/// place for `mut`/`ref` slots) or an inline-built slice-descriptor pointer.
pub(crate) enum SubscriptActual<'a> {
    Reg(Reg, Option<&'a MirPlace>),
    Descriptor(Value),
}

/// The checker-virtual slice-descriptor struct name behind `ty`, if any.
pub(crate) fn slice_struct_name(ty: &Ty) -> Option<&str> {
    match ty {
        Ty::Struct(name, _)
            if matches!(name.as_str(), "Slice" | "ContiguousSlice" | "StridedSlice") =>
        {
            Some(name)
        }
        Ty::Ref(reference) => slice_struct_name(&reference.referent),
        _ => None,
    }
}

pub(crate) fn operand_regs(instr: &MirInstr) -> Vec<Reg> {
    pub(super) fn place_regs(place: &MirPlace, out: &mut Vec<Reg>) {
        for proj in &place.proj {
            if let Proj::Index(reg) = proj {
                out.push(*reg);
            }
        }
    }
    pub(super) fn subscript_arg_regs(arg: &mojito_mir::mir::MirSubscriptArg, out: &mut Vec<Reg>) {
        match arg {
            mojito_mir::mir::MirSubscriptArg::Index(reg) => out.push(*reg),
            mojito_mir::mir::MirSubscriptArg::Slice {
                lower, upper, step, ..
            } => out.extend([lower, upper, step].into_iter().flatten()),
        }
    }
    let mut out = Vec::new();
    match instr {
        MirInstr::MaterializeLiteral { value, .. } => out.push(*value),
        MirInstr::UnOp { a, .. } => out.push(*a),
        MirInstr::BinOp { a, b, .. } => out.extend([*a, *b]),
        MirInstr::DefVar { src, .. } => out.push(*src),
        MirInstr::CopyValue { value, .. } => out.push(*value),
        MirInstr::LoadPlace { place, .. } | MirInstr::MovePlace { place, .. } => {
            place_regs(place, &mut out);
        }
        MirInstr::Store { place, src } => {
            place_regs(place, &mut out);
            out.push(*src);
        }
        MirInstr::GetField { base, .. } => out.push(*base),
        MirInstr::MakeTuple { elems, .. } => out.extend(elems.iter().copied()),
        MirInstr::Index { base, index, .. } => out.extend([*base, *index]),
        MirInstr::Slice {
            object,
            lower,
            upper,
            step,
            ..
        } => {
            out.push(*object);
            out.extend([lower, upper, step].into_iter().flatten());
        }
        MirInstr::MultiIndex {
            object,
            args,
            kwargs,
            ..
        } => {
            out.push(*object);
            for arg in args.iter().chain(kwargs.iter().map(|(_, arg)| arg)) {
                subscript_arg_regs(arg, &mut out);
            }
        }
        MirInstr::MultiSet {
            receiver,
            args,
            value,
            ..
        } => {
            out.push(*receiver);
            for arg in args {
                subscript_arg_regs(arg, &mut out);
            }
            out.push(*value);
        }
        MirInstr::ReadRef { reference, .. } => out.push(*reference),
        MirInstr::WriteRef { reference, value } => out.extend([*reference, *value]),
        MirInstr::StoreRef { place, reference } => {
            place_regs(place, &mut out);
            out.push(*reference);
        }
        MirInstr::Call { args, kwargs, .. } => {
            out.extend(args.iter().copied());
            out.extend(kwargs.iter().map(|(_, reg)| *reg));
        }
        MirInstr::CallIndirect {
            callee,
            args,
            kwargs,
            ..
        } => {
            out.push(*callee);
            out.extend(args.iter().copied());
            out.extend(kwargs.iter().map(|(_, reg)| *reg));
        }
        MirInstr::MethodCall {
            recv, args, kwargs, ..
        } => {
            out.push(*recv);
            out.extend(args.iter().copied());
            out.extend(kwargs.iter().map(|(_, reg)| *reg));
        }
        MirInstr::Raise { src } => out.push(*src),
        MirInstr::MakeVariant { value, .. }
        | MirInstr::VariantIs { variant: value, .. }
        | MirInstr::VariantGet { variant: value, .. }
        | MirInstr::VariantTake { variant: value, .. }
        | MirInstr::SimdCast { value, .. }
        | MirInstr::SimdShuffle { value, .. } => out.push(*value),
        MirInstr::VariantSet { place, value, .. }
        | MirInstr::VariantReplace { place, value, .. } => {
            place_regs(place, &mut out);
            out.push(*value);
        }
        MirInstr::VariantSetInitWith { place, factory, .. } => {
            place_regs(place, &mut out);
            out.push(*factory);
        }
        MirInstr::VariantDeinitWith {
            variant, handler, ..
        } => out.extend([*variant, *handler]),
        MirInstr::MakeSimd { elems, .. } => out.extend(elems.iter().copied()),
        _ => {}
    }
    out
}

/// Every register a terminator reads.
pub(crate) fn terminator_regs(term: &MirTerm) -> Vec<Reg> {
    match term {
        MirTerm::Branch { cond, .. } => vec![*cond],
        MirTerm::Return(Some(reg))
        | MirTerm::ReturnWithCleanup {
            value: Some(reg), ..
        } => {
            vec![*reg]
        }
        _ => Vec::new(),
    }
}

pub(crate) fn instr_name(instr: &MirInstr) -> &'static str {
    match instr {
        MirInstr::EstablishLoans { .. } => "EstablishLoans",
        MirInstr::InvalidateInteriors { .. } => "InvalidateInteriors",
        MirInstr::MakeRef { .. } => "MakeRef",
        MirInstr::ReadRef { .. } => "ReadRef",
        MirInstr::CopyValue { .. } => "CopyValue",
        MirInstr::WriteRef { .. } => "WriteRef",
        MirInstr::MakeClosure { .. } => "MakeClosure",
        MirInstr::KeepAlive { .. } => "KeepAlive",
        MirInstr::Const { .. } => "Const",
        MirInstr::SizeOf { .. } => "SizeOf",
        MirInstr::ConstructTypeParam { .. } => "ConstructTypeParam",
        MirInstr::MaterializeLiteral { .. } => "MaterializeLiteral",
        MirInstr::UseVar { .. } => "UseVar",
        MirInstr::MovePlace { .. } => "MovePlace",
        MirInstr::DefVar { .. } => "DefVar",
        MirInstr::UnOp { .. } => "UnOp",
        MirInstr::BinOp { .. } => "BinOp",
        MirInstr::Call { .. } => "Call",
        MirInstr::CallIndirect { .. } => "CallIndirect",
        MirInstr::MethodCall { .. } => "MethodCall",
        MirInstr::GetField { .. } => "GetField",
        MirInstr::Index { .. } => "Index",
        MirInstr::Slice { .. } => "Slice",
        MirInstr::MultiIndex { .. } => "MultiIndex",
        MirInstr::MultiSet { .. } => "MultiSet",
        MirInstr::Store { .. } => "Store",
        MirInstr::StoreRef { .. } => "StoreRef",
        MirInstr::LoadPlace { .. } => "LoadPlace",
        MirInstr::MakeTuple { .. } => "MakeTuple",
        MirInstr::MakeVariant { .. } => "MakeVariant",
        MirInstr::VariantIs { .. } => "VariantIs",
        MirInstr::VariantGet { .. } => "VariantGet",
        MirInstr::VariantSet { .. } => "VariantSet",
        MirInstr::VariantTake { .. } => "VariantTake",
        MirInstr::VariantSetInitWith { .. } => "VariantSetInitWith",
        MirInstr::VariantDeinitWith { .. } => "VariantDeinitWith",
        MirInstr::VariantReplace { .. } => "VariantReplace",
        MirInstr::MakeSimd { .. } => "MakeSimd",
        MirInstr::SimdCast { .. } => "SimdCast",
        MirInstr::SimdShuffle { .. } => "SimdShuffle",
        MirInstr::PointerStorageTake { .. } => "PointerStorageTake",
        MirInstr::PointerStorageDestroy { .. } => "PointerStorageDestroy",
        MirInstr::UninitStorage { .. } => "UninitStorage",
        MirInstr::UninitStorageTake { .. } => "UninitStorageTake",
        MirInstr::UninitStorageDestroy { .. } => "UninitStorageDestroy",
        MirInstr::Raise { .. } => "Raise",
        MirInstr::Try { .. } => "Try",
        MirInstr::Drop { .. } => "Drop",
        MirInstr::DropVar { .. } => "DropVar",
        MirInstr::ConsumeVar { .. } => "ConsumeVar",
        MirInstr::ConsumePlace { .. } => "ConsumePlace",
        MirInstr::GetIter { .. } => "GetIter",
        MirInstr::HasNext { .. } => "HasNext",
        MirInstr::Next { .. } => "Next",
        MirInstr::TryNext { .. } => "TryNext",
        MirInstr::Unsupported(_) => "Unsupported",
    }
}

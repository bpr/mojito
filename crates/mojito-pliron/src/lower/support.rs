//! Free helpers: thunk/pow bodies, C-ABI types, MIR scans, scalar
//! predicates, and instruction metadata.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

/// Emit the body of an `invoke` thunk (see [`ModuleShared::ensure_thunk`]):
/// load/take the capture arguments out of the environment record, forward
/// the out-pointer and every user argument unchanged, call the lifted
/// target directly, and return its result.
pub fn emit_thunk_body(
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

/// The LLVM-dialect type of one runtime-contract primitive. LLVM integers
/// are signless, so `U64` and `I64` share `i64`; pointers are opaque.
pub fn c_abi_type(ctx: &Context, ty: mojito_native::native::rt_abi::CAbiTy) -> TypeHandle {
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
pub fn collect_projected_move_places<'m>(
    blocks: &'m [mojito_mir::mir::MirBlock],
    out: &mut Vec<&'m MirPlace>,
) {
    for block in blocks {
        for instr in &block.instrs {
            match instr {
                MirInstr::MovePlace { place, .. } if !place.proj.is_empty() => out.push(place),
                MirInstr::DropPlace { place } => out.push(place),
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

/// The `LoadPlace` results that alias their place instead of materializing a
/// copy: an aggregate register whose every consumer borrows it. MIR's
/// aggregate load is a shallow read whose owner stays live through every
/// consumer, so the address is valid wherever the register is read; a
/// callable is excluded because its owner is retained for one hop only. Any
/// owning or unclassified consumer keeps the copy — the conservative side is
/// an extra copy, never two owners of one buffer.
pub fn collect_aliased_load_regs(
    function: &MirFunction,
    declarations: &HashMap<String, MirFunctionDeclaration>,
) -> HashSet<u32> {
    fn visit(
        blocks: &[MirBlock],
        declarations: &HashMap<String, MirFunctionDeclaration>,
        owning: &mut HashSet<u32>,
    ) {
        for block in blocks {
            for instruction in &block.instrs {
                let borrowed = borrowed_operands(instruction, declarations);
                let mut operands = Vec::new();
                mojito_mir::mir::verify::instruction_operand_regs(instruction, &mut operands);
                owning.extend(
                    operands
                        .iter()
                        .map(|reg| reg.0)
                        .filter(|reg| !borrowed.contains(reg)),
                );
                if let MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } = instruction
                {
                    visit(body, declarations, owning);
                    if let Some((_, blocks)) = handler {
                        visit(blocks, declarations, owning);
                    }
                    if let Some(blocks) = orelse {
                        visit(blocks, declarations, owning);
                    }
                    if let Some(blocks) = finalbody {
                        visit(blocks, declarations, owning);
                    }
                }
            }
            owning.extend(terminator_regs(&block.term).iter().map(|reg| reg.0));
        }
    }

    let mut owning = HashSet::new();
    visit(&function.blocks, declarations, &mut owning);
    collect_loaded_places(&function.blocks)
        .into_keys()
        .filter(|reg| !owning.contains(reg))
        .filter(|reg| {
            function
                .reg_types
                .get(reg)
                .is_some_and(|ty| is_aggregate_ty(ty) && !matches!(ty, Ty::Func { .. }))
        })
        .collect()
}

pub fn collect_loaded_places(blocks: &[MirBlock]) -> HashMap<u32, MirPlace> {
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
pub const fn scalar_copy_ty(ty: &Ty) -> Option<ScalarTy> {
    match ty {
        Ty::Int | Ty::IntLiteral => Some(ScalarTy::Int),
        Ty::UInt => Some(ScalarTy::UInt),
        Ty::Float64 | Ty::FloatLiteral => Some(ScalarTy::Float64),
        Ty::Bool => Some(ScalarTy::Bool),
        Ty::Dtype => Some(ScalarTy::Dtype),
        ty => match scalar_simd_dtype(ty) {
            Some(dtype) => Some(ScalarTy::of_dtype(dtype)),
            None => None,
        },
    }
}

/// The concrete lane dtype and width of a `Ty::Simd` register type. A
/// symbolic slot never reaches lowering: the MIR verifier refuses it.
pub fn simd_dims(ty: &Ty) -> Option<(Dtype, i64)> {
    match ty {
        Ty::Simd { dtype, width } => Some((dtype.known()?, width.known()?)),
        _ => None,
    }
}

pub const fn is_aggregate_ty(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Struct(..) | Ty::Tuple(..) | Ty::RuntimePack(..) | Ty::Variant(..) | Ty::Func { .. }
    )
}

/// Map a checked type to its scalar lowering, or reject it.
pub fn scalar_type(
    function: &str,
    ty: &Ty,
    location: Option<SourceSpan>,
) -> Result<ScalarTy, PlironError> {
    match ty {
        Ty::Int => Ok(ScalarTy::Int),
        Ty::UInt => Ok(ScalarTy::UInt),
        Ty::Float64 => Ok(ScalarTy::Float64),
        Ty::Bool => Ok(ScalarTy::Bool),
        Ty::Dtype => Ok(ScalarTy::Dtype),
        ty if let Some(dtype) = scalar_simd_dtype(ty) => Ok(ScalarTy::of_dtype(dtype)),
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

pub const fn is_comparison(op: InfixOp) -> bool {
    matches!(
        op,
        InfixOp::Eq | InfixOp::Ne | InfixOp::Lt | InfixOp::Le | InfixOp::Gt | InfixOp::Ge
    )
}

pub fn signed_predicate(op: InfixOp) -> ICmpPredicateAttr {
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

pub fn unsigned_predicate(op: InfixOp) -> ICmpPredicateAttr {
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

pub fn float_predicate(op: InfixOp) -> FCmpPredicateAttr {
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

pub const fn no_overflow_flags() -> IntegerOverflowFlagsAttr {
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
pub fn record_last_uses(
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
pub enum SubscriptActual<'a> {
    Reg(Reg, Option<&'a MirPlace>),
    Descriptor(Value),
}

/// The checker-virtual slice-descriptor struct name behind `ty`, if any.
pub fn slice_struct_name(ty: &Ty) -> Option<&str> {
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

pub fn operand_regs(instr: &MirInstr) -> Vec<Reg> {
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
        | MirInstr::SimdBitcast { value, .. } => out.push(*value),
        MirInstr::SimdShuffle { value, other, .. } => {
            out.push(*value);
            out.extend(*other);
        }
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
pub fn terminator_regs(term: &MirTerm) -> Vec<Reg> {
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

pub const fn instr_name(instr: &MirInstr) -> &'static str {
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
        MirInstr::SimdBitcast { .. } => "SimdBitcast",
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
        MirInstr::DropPlace { .. } => "DropPlace",
        MirInstr::GetIter { .. } => "GetIter",
        MirInstr::HasNext { .. } => "HasNext",
        MirInstr::Next { .. } => "Next",
        MirInstr::TryNext { .. } => "TryNext",
        MirInstr::Unsupported(_) => "Unsupported",
    }
}

/// The operand registers `instruction` only borrows: read receivers and
/// non-consuming argument slots of declared callees, the operands of the
/// value-reading builtins, and the bases of field, variant and subscript
/// reads. Every other operand position takes, moves, stores or mutates its
/// register.
fn borrowed_operands(
    instruction: &MirInstr,
    declarations: &HashMap<String, MirFunctionDeclaration>,
) -> Vec<u32> {
    match instruction {
        MirInstr::CopyValue { value, .. }
        | MirInstr::GetField { base: value, .. }
        | MirInstr::VariantIs { variant: value, .. }
        | MirInstr::VariantGet { variant: value, .. } => vec![value.0],
        MirInstr::Index {
            base,
            base_place,
            call,
            ..
        } => match call {
            None => vec![base.0],
            Some(call) if receiver_borrows(call.receiver_convention, base_place.is_some()) => {
                vec![base.0]
            }
            Some(_) => Vec::new(),
        },
        MirInstr::Slice {
            object,
            object_place,
            call: Some(call),
            ..
        }
        | MirInstr::MultiIndex {
            object,
            object_place,
            call: Some(call),
            ..
        } if receiver_borrows(call.receiver_convention, object_place.is_some()) => {
            vec![object.0]
        }
        MirInstr::MultiSet {
            receiver,
            receiver_place,
            call,
            ..
        } if receiver_borrows(call.receiver_convention, receiver_place.is_some()) => {
            vec![receiver.0]
        }
        MirInstr::MethodCall {
            recv,
            resolved: Some(resolved),
            recv_place,
            args,
            kwargs,
            ..
        } => {
            let Some(declaration) = declarations.get(resolved) else {
                return Vec::new();
            };
            if declaration.variadic.is_some() || declaration.kw_variadic.is_some() {
                return Vec::new();
            }
            let mut borrowed = borrowed_slots(declaration, args, kwargs);
            if receiver_borrows(declaration.receiver_convention, recv_place.is_some()) {
                borrowed.push(recv.0);
            }
            borrowed
        }
        MirInstr::MethodCall {
            resolved: None,
            method,
            args,
            ..
        } if matches!(method.as_str(), "write" | "write_string") => {
            args.iter().map(|reg| reg.0).collect()
        }
        MirInstr::Call {
            func,
            args,
            kwargs,
            kwarg_places,
            ..
        } => {
            let mut borrowed = match declarations.get(&func.0) {
                Some(declaration)
                    if declaration.variadic.is_none() && declaration.kw_variadic.is_none() =>
                {
                    borrowed_slots(declaration, args, kwargs)
                }
                Some(_) => Vec::new(),
                None if matches!(
                    func.0.as_str(),
                    "print"
                        | "String"
                        | "repr"
                        | "len"
                        | "abs"
                        | "round"
                        | "Int"
                        | "Float64"
                        | "Bool"
                ) =>
                {
                    args.iter()
                        .chain(kwargs.iter().map(|(_, reg)| reg))
                        .map(|reg| reg.0)
                        .collect()
                }
                None => Vec::new(),
            };
            // `Type(copy=place)` is the explicit copy-constructor boundary:
            // the constructor itself runs the copy lifecycle on the borrowed
            // source.
            borrowed.extend(
                kwargs
                    .iter()
                    .zip(kwarg_places)
                    .filter(|((name, _), place)| name == "copy" && place.is_some())
                    .map(|((_, reg), _)| reg.0),
            );
            borrowed
        }
        _ => Vec::new(),
    }
}

/// The positional and keyword arguments bound to non-consuming slots of
/// `declaration`. `param_conventions` covers the explicit parameters only,
/// so positional index `i` is slot `i`.
fn borrowed_slots(
    declaration: &MirFunctionDeclaration,
    args: &[Reg],
    kwargs: &[(String, Reg)],
) -> Vec<u32> {
    let slot_borrows = |index: usize| {
        !matches!(
            declaration.param_conventions.get(index).copied().flatten(),
            Some(mojito_ast::ast::ArgConvention::Var | mojito_ast::ast::ArgConvention::Deinit)
        )
    };
    let positional = args
        .iter()
        .enumerate()
        .filter(|(index, _)| slot_borrows(*index))
        .map(|(_, reg)| reg.0);
    let keyword = kwargs.iter().filter_map(|(name, reg)| {
        declaration
            .param_names
            .iter()
            .position(|parameter| parameter == name)
            .filter(|index| slot_borrows(*index))
            .map(|_| reg.0)
    });
    positional.chain(keyword).collect()
}

/// Whether a receiver of `convention` only borrows its register. A `mut` or
/// `deinit` receiver borrows through its retained place (the call addresses
/// the place directly); without one it takes a copy that the write-back or
/// the destructor consumes.
const fn receiver_borrows(
    convention: Option<mojito_ast::ast::ArgConvention>,
    has_place: bool,
) -> bool {
    match convention {
        Some(mojito_ast::ast::ArgConvention::Var | mojito_ast::ast::ArgConvention::Out) => false,
        Some(mojito_ast::ast::ArgConvention::Mut | mojito_ast::ast::ArgConvention::Deinit) => {
            has_place
        }
        _ => true,
    }
}

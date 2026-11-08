//! Free helpers: thunk bodies, C-ABI types, MIR scans, scalar
//! predicates, and instruction metadata.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::{SimdDtype, SimdWidth, TyKind};

/// Emit the body of an `invoke` thunk (see [`ModuleShared::ensure_thunk`]):
/// load the capture arguments out of the environment record, forward
/// the out-pointer and every user argument unchanged, call the lifted
/// target directly, and return its result. An initializer's `out self`
/// receiver is the thunk's out-pointer, or storage the thunk reads its
/// scalar result back from ([`ThunkReceiver`]).
pub fn emit_thunk_body(
    ctx: &mut Context,
    func: FuncOp,
    target: &FnSignature,
    (captures, receiver): (usize, ThunkReceiver),
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
    let out = has_out.then(|| argument(ctx, &mut next_argument));
    if receiver != ThunkReceiver::OutPointer {
        operands.extend(out);
    }
    let env = argument(ctx, &mut next_argument);
    // Each slot stores a captured place's address, which the lifted body
    // takes as a leading reference parameter.
    for index in 0..captures {
        let slot = if index == 0 {
            env
        } else {
            let offset = u32::try_from(index * 8).expect("environment offsets fit u32");
            let gep = GetElementPtrOp::new(ctx, env, vec![GepIndex::Constant(offset)], i8_ty);
            gep.get_operation().insert_at_back(entry, ctx);
            gep.get_result(ctx)
        };
        let load = LoadOp::new(ctx, slot, ptr_ty);
        load.get_operation().insert_at_back(entry, ctx);
        operands.push(load.get_result(ctx));
    }
    let storage = match receiver {
        ThunkReceiver::Forwarded => None,
        ThunkReceiver::OutPointer => out,
        // Every native scalar fits one 8-byte slot.
        ThunkReceiver::Scalar(_) => {
            let i64_int = IntegerType::get(ctx, 64, Signedness::Signless);
            let attr = IntegerAttr::new(i64_int, APInt::from_u64(8, bw(64)));
            let count = ConstantOp::new(ctx, Box::new(attr));
            count.get_operation().insert_at_back(entry, ctx);
            let alloca = AllocaOp::new(ctx, i8_ty, count.get_result(ctx), 0);
            alloca.set_alignment(ctx, 8);
            alloca.get_operation().insert_at_back(entry, ctx);
            Some(alloca.get_result(ctx))
        }
    };
    operands.extend(storage);
    let mut remaining = target
        .params
        .iter()
        .enumerate()
        .skip(captures + usize::from(receiver != ThunkReceiver::Forwarded))
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
    let result = match (receiver, storage) {
        (ThunkReceiver::Scalar(scalar), Some(storage)) => {
            let handle = scalar.handle(ctx);
            let load = LoadOp::new(ctx, storage, handle);
            load.get_operation().insert_at_back(entry, ctx);
            Some(load.get_result(ctx))
        }
        _ => target.returns_value.then(|| call.get_result(ctx)),
    };
    let ret = ReturnOp::new(ctx, result);
    ret.get_operation().insert_at_back(entry, ctx);
}

/// What an `invoke` thunk passes as its target's first parameter: the
/// first forwarded argument, or, for a function value of an initializer,
/// the `out self` receiver its result is, as upstream's `__init__` returns
/// the struct it initializes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ThunkReceiver {
    /// Not an initializer: every parameter forwards unchanged.
    Forwarded,
    /// An aggregate receiver: the thunk's sret out-pointer.
    OutPointer,
    /// A receiver held as a scalar: storage the thunk returns the value of.
    Scalar(ScalarTy),
}

impl ThunkReceiver {
    /// The receiver a function value of `target`, closing over `captures`
    /// places, passes. A zero-sized receiver passes nothing, so it
    /// forwards.
    pub fn of(target: &FnSignature, captures: usize) -> Self {
        if !target.out_receiver || captures > 0 {
            return Self::Forwarded;
        }
        match target.params.first() {
            Some(LowerTy::Aggregate { .. }) => Self::OutPointer,
            Some(LowerTy::Scalar(scalar)) => Self::Scalar(*scalar),
            _ => Self::Forwarded,
        }
    }
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

/// The `LoadPlace` results that alias their place instead of materializing a
/// copy: an aggregate register whose every consumer borrows it. MIR's
/// aggregate load is a shallow read whose owner stays live through every
/// consumer, so the address is valid wherever the register is read; a
/// callable is excluded because its owner is retained for one hop only, and
/// a held struct is its field ([`MirDeclarations::translated_type`]). Any
/// owning or unclassified consumer keeps the byte snapshot, which no later
/// store to the place reaches; the MIR names the copy an owner needs. A load of a
/// register-passable trivial value is that value, as upstream's
/// `lit.ref.load` is, so it always copies out: a forwarded copy
/// ([`MirDeclarations::copy_is_identity`]) reads the register in the copy's
/// place, and must not see a later store to the place.
pub fn collect_aliased_load_regs(
    function: &MirFunction,
    declarations: &HashMap<String, MirFunctionDeclaration>,
    mir_declarations: &MirDeclarations,
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
                for region in mojito_mir::mir::instruction_regions(instruction) {
                    visit(region, declarations, owning);
                }
            }
            owning.extend(
                mojito_mir::mir::terminator_regs(&block.term)
                    .iter()
                    .map(|reg| reg.0),
            );
        }
    }

    let mut owning = HashSet::new();
    visit(&function.blocks, declarations, &mut owning);
    collect_loaded_places(&function.blocks)
        .into_keys()
        .filter(|reg| !owning.contains(reg))
        .filter(|reg| {
            function.reg_types.get(reg).is_some_and(|ty| {
                is_aggregate_ty(&mir_declarations.translated_type(ty))
                    && !matches!(ty.kind(), TyKind::Func(_))
                    && !mir_declarations.copy_is_identity(ty)
            })
        })
        .collect()
}

pub fn collect_loaded_places(blocks: &[MirBlock]) -> HashMap<u32, MirPlace> {
    pub(super) fn visit(blocks: &[MirBlock], output: &mut HashMap<u32, MirPlace>) {
        for block in blocks {
            for instruction in &block.instrs {
                if let MirInstr::LoadPlace { dest, place } = instruction {
                    output.insert(dest.0, place.clone());
                }
                for region in mojito_mir::mir::instruction_regions(instruction) {
                    visit(region, output);
                }
            }
        }
    }
    let mut output = HashMap::new();
    visit(blocks, &mut output);
    output
}

/// The concrete lane dtype and width of a register type as LLVM
/// translation sees it: a `SIMD` struct is its `!kgen.simd` storage
/// (`types::storage_ty`), and the builtin `index` is held as the `Int` it
/// casts from. A symbolic slot never reaches lowering: the MIR verifier
/// refuses it.
pub fn simd_dims(ty: &Ty) -> Option<(Dtype, i64)> {
    match mojito_types::types::storage_ty(ty).kind() {
        TyKind::Index => Some((Dtype::Int, 1)),
        kind => {
            let (dtype, width) = mojito_types::types::kgen_simd_slots(kind)?;
            Some((dtype.known()?, width.known()?))
        }
    }
}

pub fn is_aggregate_ty(ty: &Ty) -> bool {
    ty.is_non_simd_struct()
        || matches!(
            ty.kind(),
            TyKind::Tuple(..)
                | TyKind::PopArray { .. }
                | TyKind::RuntimePack(..)
                | TyKind::Variant(..)
                | TyKind::Func(_)
        )
}

/// Map a checked type to its scalar lowering, or reject it.
pub fn scalar_type(
    function: &str,
    ty: &Ty,
    location: Option<SourceSpan>,
) -> Result<ScalarTy, PlironError> {
    match ty.kind() {
        kind if kind.is_bool() => Ok(ScalarTy::Bool),
        TyKind::I1 => Ok(ScalarTy::Bool),
        TyKind::KgenDtype => Ok(ScalarTy::Dtype),
        TyKind::Ui8 => Ok(ScalarTy::of_dtype(mojito_ast::ast::Dtype::UInt8)),
        TyKind::Index => Ok(ScalarTy::Int),
        kind if kind.is_simd() => {
            scalar_type(function, &mojito_types::types::storage_ty(ty), location)
        }
        TyKind::KgenSimd {
            dtype: SimdDtype::Known(dtype),
            width: SimdWidth::Known(1),
        } => Ok(ScalarTy::of_dtype(*dtype)),
        TyKind::Pointer { .. } | TyKind::Ref(_) => Ok(ScalarTy::Ptr),
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

/// The LLVM fast-math flags of a `pop` op's closed `fastmathFlags`
/// attribute (`#pop.fmf<nnan | ninf>`), as upstream's LLVM lowering copies
/// the property onto the instruction; none when the op has no attribute.
pub fn fastmath_attr(fastmath: Option<&MirConst>) -> FastmathFlagsAttr {
    use mojito_types::mlir::FastMathFlags as Flags;
    use pliron_llvm::attributes::FastmathFlags as Llvm;
    let set = fastmath
        .and_then(|flag| match flag {
            MirConst::Value(mojito_types::ct::CtValue::MlirAttr(attribute)) => {
                Flags::parse(attribute)
            }
            _ => None,
        })
        .unwrap_or_default();
    let llvm = [
        (Flags::NNAN, Llvm::NNAN),
        (Flags::NINF, Llvm::NINF),
        (Flags::NSZ, Llvm::NSZ),
        (Flags::ARCP, Llvm::ARCP),
        (Flags::CONTRACT, Llvm::CONTRACT),
        (Flags::AFN, Llvm::AFN),
        (Flags::REASSOC, Llvm::REASSOC),
    ]
    .into_iter()
    .filter(|(flag, _)| set.contains(*flag))
    .fold(Llvm::empty(), |flags, (_, flag)| flags | flag);
    FastmathFlagsAttr(llvm)
}

pub fn float_predicate(op: InfixOp) -> FCmpPredicateAttr {
    match op {
        InfixOp::Eq => FCmpPredicateAttr::OEQ,
        // `pop.cmp ne` is unordered: a NaN operand is unequal to everything.
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

/// One already-lowered subscript actual: an index register, with its checked
/// place for `mut`/`ref` slots.
pub enum SubscriptActual<'a> {
    Reg(Reg, Option<&'a MirPlace>),
}

pub const fn instr_name(instr: &MirInstr) -> &'static str {
    match instr {
        MirInstr::EstablishLoans { .. } => "EstablishLoans",
        MirInstr::InvalidateInteriors { .. } => "InvalidateInteriors",
        MirInstr::MakeRef { .. } => "MakeRef",
        MirInstr::ReadRef { .. } => "ReadRef",
        MirInstr::CopyValue { .. } => "CopyValue",
        MirInstr::Rebind { .. } => "Rebind",
        MirInstr::WriteRef { .. } => "WriteRef",
        MirInstr::MakeClosure { .. } => "MakeClosure",
        MirInstr::KeepAlive { .. } => "KeepAlive",
        MirInstr::Const { .. } => "Const",
        MirInstr::ParamListAddress { .. } => "ParamListAddress",
        MirInstr::StoreToMem { .. } => "StoreToMem",
        MirInstr::ConstructTypeParam { .. } => "ConstructTypeParam",
        MirInstr::ConstructType { .. } => "ConstructType",
        MirInstr::MaterializeLiteral { .. } => "MaterializeLiteral",
        MirInstr::ScalarCast { .. } => "ScalarCast",
        MirInstr::BuiltinCast { .. } => "BuiltinCast",
        MirInstr::PointerConvert { .. } => "PointerConvert",
        MirInstr::Pop { .. } => "Pop",
        MirInstr::IsRunInComptimeInterpreter { .. } => "IsRunInComptimeInterpreter",
        MirInstr::CallLlvmIntrinsic { .. } => "CallLlvmIntrinsic",
        MirInstr::UseVar { .. } => "UseVar",
        MirInstr::MovePlace { .. } => "MovePlace",
        MirInstr::DefVar { .. } => "DefVar",
        MirInstr::DeclareVar { .. } => "DeclareVar",
        MirInstr::UnOp { .. } => "UnOp",
        MirInstr::BinOp { .. } => "BinOp",
        MirInstr::Call { .. } => "Call",
        MirInstr::CallIndirect { .. } => "CallIndirect",
        MirInstr::MethodCall { .. } => "MethodCall",
        MirInstr::GetField { .. } => "GetField",
        MirInstr::Index { .. } => "Index",
        MirInstr::MultiIndex { .. } => "MultiIndex",
        MirInstr::MultiSet { .. } => "MultiSet",
        MirInstr::Store { .. } => "Store",
        MirInstr::StoreRef { .. } => "StoreRef",
        MirInstr::LoadPlace { .. } => "LoadPlace",
        MirInstr::MakeTuple { .. } => "MakeTuple",
        MirInstr::MakeStruct { .. } => "MakeStruct",
        MirInstr::MakeArray { .. } => "MakeArray",
        MirInstr::MakeRefPack { .. } => "MakeRefPack",
        MirInstr::LoadIndirect { .. } => "LoadIndirect",
        MirInstr::ArrayRepeat { .. } => "ArrayRepeat",
        MirInstr::ArrayGet { .. } => "ArrayGet",
        MirInstr::ArrayReplace { .. } => "ArrayReplace",
        MirInstr::ArrayGep { .. } => "ArrayGep",
        MirInstr::MakeVariant { .. } => "MakeVariant",
        MirInstr::VariantIs { .. } => "VariantIs",
        MirInstr::VariantTag { .. } => "VariantTag",
        MirInstr::VariantGet { .. } => "VariantGet",
        MirInstr::VariantSet { .. } => "VariantSet",
        MirInstr::VariantTake { .. } => "VariantTake",
        MirInstr::VariantReplace { .. } => "VariantReplace",
        MirInstr::MakeSimd { .. } => "MakeSimd",
        MirInstr::SimdCast { .. } => "SimdCast",
        MirInstr::SimdBitcast { .. } => "SimdBitcast",
        MirInstr::SimdShuffle { .. } => "SimdShuffle",
        MirInstr::Select { .. } => "Select",
        MirInstr::SimdSelect { .. } => "SimdSelect",
        MirInstr::SimdExtract { .. } => "SimdExtract",
        MirInstr::SimdInsert { .. } => "SimdInsert",
        MirInstr::SimdReduce { .. } => "SimdReduce",
        MirInstr::PointerStorageTake { .. } => "PointerStorageTake",
        MirInstr::PointerStorageDestroy { .. } => "PointerStorageDestroy",
        MirInstr::UninitStorage { .. } => "UninitStorage",
        MirInstr::UninitStorageTake { .. } => "UninitStorageTake",
        MirInstr::UninitStorageDestroy { .. } => "UninitStorageDestroy",
        MirInstr::Try { .. } => "Try",
        MirInstr::If { .. } => "If",
        MirInstr::Loop { .. } => "Loop",
        MirInstr::ComptimeIf { .. } => "ComptimeIf",
        MirInstr::ComptimeFor { .. } => "ComptimeFor",
        MirInstr::ComptimeAssert { .. } => "ComptimeAssert",
        MirInstr::Match { .. } => "Match",
        MirInstr::Drop { .. } => "Drop",
        MirInstr::DropVar { .. } => "DropVar",
        MirInstr::ConsumeVar { .. } => "ConsumeVar",
        MirInstr::DropPlace { .. } => "DropPlace",
        MirInstr::MarkInitialized { .. } => "MarkInitialized",
        MirInstr::MarkDestroyed { .. } => "MarkDestroyed",
        MirInstr::GetIter { .. } => "GetIter",
        MirInstr::HasNext { .. } => "HasNext",
        MirInstr::Next { .. } => "Next",
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
        | MirInstr::VariantTag { variant: value, .. }
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
        MirInstr::MultiIndex {
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
            if declaration.kw_variadic.is_some() {
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
            // A construction of a struct with one `__init__` names the
            // struct; its arguments bind by that initializer's conventions.
            let declaration = declarations
                .get(&func.0)
                .or_else(|| declarations.get(&format!("{}.__init__", func.0)));
            let mut borrowed = match declaration {
                Some(declaration) if declaration.kw_variadic.is_none() => {
                    borrowed_slots(declaration, args, kwargs)
                }
                Some(_) => Vec::new(),
                None if matches!(func.0.as_str(), "len" | "abs") => args
                    .iter()
                    .chain(kwargs.iter().map(|(_, reg)| reg))
                    .map(|reg| reg.0)
                    .collect(),
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
/// so positional index `i` is slot `i` up to the positional collector, which
/// gathers every later one under its own convention.
fn borrowed_slots(
    declaration: &MirFunctionDeclaration,
    args: &[Reg],
    kwargs: &[(String, Reg)],
) -> Vec<u32> {
    let borrows = |convention: Option<mojito_ast::ast::ArgConvention>| {
        !matches!(
            convention,
            Some(mojito_ast::ast::ArgConvention::Var | mojito_ast::ast::ArgConvention::Deinit)
        )
    };
    let slot_borrows =
        |index: usize| borrows(declaration.param_conventions.get(index).copied().flatten());
    let collected_from = declaration
        .variadic
        .as_ref()
        .map(|_| declaration.variadic_index.unwrap_or(0));
    let positional = args
        .iter()
        .enumerate()
        .filter(|(index, _)| match collected_from {
            Some(first) if *index >= first => borrows(declaration.variadic_convention),
            _ => slot_borrows(*index),
        })
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

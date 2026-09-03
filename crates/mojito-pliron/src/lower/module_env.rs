//! Module-wide lowering environment: compiled-signature construction
//! ([`ModuleShared`] helpers), [`declare_function`], the synthesized `main`
//! exe wrapper, and the intercepted-call filter.

use super::*;

impl ModuleShared {
    pub(crate) fn new(module: ModuleOp) -> ModuleShared {
        ModuleShared {
            module,
            rt_types: HashMap::new(),
            strings: HashMap::new(),
            pow_ty: None,
            thunks: HashMap::new(),
            drop_thunks: HashMap::new(),
        }
    }

    /// Whether body lowering already declared the runtime symbol (the
    /// executable wrapper must not redeclare it).
    pub(crate) fn declared_rt(&self, symbol: &str) -> bool {
        self.rt_types.contains_key(symbol)
    }

    /// Intern `bytes` as a private constant-pool global `mjstr_<n>` —
    /// deduplicated by content and numbered in first-interning order, so
    /// emission is deterministic — and return its symbol. The bytes are the
    /// whole constant: UTF-8, not NUL-terminated (`MjStrDesc` carries the
    /// length).
    pub(super) fn intern_string(&mut self, ctx: &mut Context, bytes: &[u8]) -> Identifier {
        if let Some(name) = self.strings.get(bytes) {
            return name.clone();
        }
        let name: Identifier = format!("mjstr_{}", self.strings.len())
            .try_into()
            .expect("constant-pool names are identifier-safe");
        let i8_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
        let array_ty: TypeHandle = ArrayType::get(ctx, i8_ty, bytes.len() as u64).into();
        let global = GlobalOp::new(ctx, name.clone(), array_ty);
        global.set_initializer_value(ctx, Box::new(BytesAttr::new(bytes.to_vec())));
        global.set_attr_llvm_global_linkage(ctx, LinkageAttr::PrivateLinkage);
        self.module.append_operation(ctx, global.get_operation(), 0);
        self.strings.insert(bytes.to_vec(), name.clone());
        name
    }

    /// Declare the runtime-contract symbol `symbol` (a `rt_abi::RT_SYMBOLS`
    /// row) once and return its call type. The declared LLVM signature comes
    /// mechanically from the contract table's `CAbiTy` row.
    pub(super) fn ensure_rt(
        &mut self,
        ctx: &mut Context,
        symbol: &'static str,
    ) -> TypedHandle<FuncType> {
        if let Some(ty) = self.rt_types.get(symbol) {
            return *ty;
        }
        let sig = mojito_native::native::rt_abi::find_symbol(symbol)
            .unwrap_or_else(|| panic!("`{symbol}` is not in the runtime contract table"));
        let ret = match sig.ret {
            Some(ty) => c_abi_type(ctx, ty),
            None => VoidType::get(ctx).to_handle(),
        };
        let mut params = Vec::with_capacity(sig.params.len());
        for (_, ty) in sig.params {
            params.push(c_abi_type(ctx, *ty));
        }
        let func_ty = FuncType::get(ctx, ret, params, false);
        let identifier: Identifier = sig
            .symbol
            .try_into()
            .expect("runtime symbols are identifier-safe");
        let func = FuncOp::new(ctx, identifier, func_ty);
        self.module.append_operation(ctx, func.get_operation(), 0);
        self.rt_types.insert(symbol, func_ty);
        func_ty
    }

    /// Emit the wrapping square-and-multiply `mjrt_pow(base, exp) -> i64`
    /// once and return its call type. Callers guard the exponent range first;
    /// wrapping i64 multiplication is bit-identical for Int and UInt, so one
    /// helper serves both.
    pub(super) fn ensure_pow(&mut self, ctx: &mut Context) -> TypedHandle<FuncType> {
        if let Some(ty) = self.pow_ty {
            return ty;
        }
        let i64_ty: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let pow_ty = FuncType::get(ctx, i64_ty, vec![i64_ty, i64_ty], false);
        let func = FuncOp::new(
            ctx,
            "mjrt_pow".try_into().expect("valid identifier"),
            pow_ty,
        );
        self.module.append_operation(ctx, func.get_operation(), 0);
        emit_pow_body(ctx, func);
        self.pow_ty = Some(pow_ty);
        pow_ty
    }

    /// Intern the `invoke` thunk adapting compiled `target` to the uniform
    /// indirect-call ABI and return its symbol. `modes` encodes the leading
    /// capture parameters (`r` reference / `c` copy / `m` move — empty for a
    /// bare function value); `capture_offsets` gives each capture's byte
    /// offset in the environment record. The thunk takes
    /// `[outcome*|sret*], env*, params...`, rebuilds the target's leading
    /// capture arguments from the record — a `Reference` slot holds the
    /// captured place's address (loaded), an owned slot holds the value
    /// inline (its address is the argument, since every capture parameter is
    /// a reference parameter) — and forwards everything else unchanged,
    /// out-pointer included, so a raising target's tagged outcome flows
    /// through untouched.
    pub(super) fn ensure_thunk(
        &mut self,
        ctx: &mut Context,
        target: &FnSignature,
        modes: &str,
        capture_offsets: &[u64],
    ) -> Identifier {
        let key = (target.mangled.clone(), modes.to_string());
        if let Some(name) = self.thunks.get(&key) {
            return name.clone();
        }
        let name: Identifier = format!("mjthunk_{}", self.thunks.len())
            .try_into()
            .expect("thunk names are identifier-safe");
        let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
        let captures = modes.len();
        let has_out = target.sret.is_some() || target.outcome.is_some();
        let mut param_handles: Vec<TypeHandle> = Vec::new();
        if has_out {
            param_handles.push(ptr_ty);
        }
        param_handles.push(ptr_ty);
        for (index, param) in target.params.iter().enumerate().skip(captures) {
            let by_reference = target.ref_params.get(index).copied().unwrap_or(false);
            match param {
                _ if by_reference => param_handles.push(ptr_ty),
                LowerTy::Scalar(scalar) => param_handles.push(scalar.handle(ctx)),
                LowerTy::Aggregate { .. } => param_handles.push(ptr_ty),
                LowerTy::ZeroSized => {}
            }
        }
        let result = target.func_ty.deref(ctx).result_type();
        let thunk_ty = FuncType::get(ctx, result, param_handles, false);
        let func = FuncOp::new(ctx, name.clone(), thunk_ty);
        self.module.append_operation(ctx, func.get_operation(), 0);
        emit_thunk_body(ctx, func, target, modes, capture_offsets, has_out);
        self.thunks.insert(key, name.clone());
        name
    }
}

/// Convert a function's checked signature and append an empty `llvm.func`
/// shell to `module`. Scalars pass and return by value; aggregates pass by
/// pointer and return through a prepended sret out-pointer.
pub(crate) fn declare_function(
    ctx: &mut Context,
    module: ModuleOp,
    name: &str,
    func: &MirFunction,
    layout: &LayoutCx<'_>,
) -> Result<(FuncOp, FnSignature), PlironError> {
    let ret_ty = func.ret_ty.as_ref().ok_or_else(|| PlironError {
        function: Some(name.to_string()),
        kind: PlironErrorKind::Unsupported {
            construct: "function without a recorded return type".into(),
        },
        location: None,
    })?;
    let (result, returns_value, ret, sret, outcome) = if func.returns_reference {
        if func.raises {
            // A raising reference return rides the tagged outcome with a
            // single place pointer as the ok payload (the reference-yielding
            // raising `__next__` convention).
            let pointer = Ty::Pointer {
                element: Box::new(ret_ty.clone()),
                origin: mojito_types::origin::PointerOrigin::Untracked { mutable: true },
            };
            let composed = layout
                .outcome_layout(&pointer)
                .map_err(|error| PlironError {
                    function: Some(name.to_string()),
                    kind: PlironErrorKind::Unsupported {
                        construct: format!("raising reference return ({error})"),
                    },
                    location: None,
                })?;
            let outcome = OutcomeAbi {
                layout: composed.layout,
                ok_offset: composed.offsets[1],
                err_offset: composed.offsets[2],
                ok: LowerTy::Scalar(ScalarTy::Ptr),
                ok_is_reference: true,
            };
            (
                VoidType::get(ctx).to_handle(),
                false,
                RetKind::Void,
                None,
                Some(outcome),
            )
        } else {
            // A reference returns as one pointer to caller-owned referent
            // storage; the checked return type names the referent.
            (
                PointerType::get(ctx, 0).into(),
                true,
                RetKind::Ptr,
                None,
                None,
            )
        }
    } else if func.raises {
        // A raising function returns `void` through a prepended outcome
        // out-pointer; its ok payload (any kind) lives inline in the outcome.
        let ok = lower_ty(name, ret_ty, layout, None)?;
        let composed = layout.outcome_layout(ret_ty).map_err(|error| PlironError {
            function: Some(name.to_string()),
            kind: PlironErrorKind::Unsupported {
                construct: format!("raising return of `{ret_ty}` ({error})"),
            },
            location: None,
        })?;
        let outcome = OutcomeAbi {
            layout: composed.layout,
            ok_offset: composed.offsets[1],
            err_offset: composed.offsets[2],
            ok,
            ok_is_reference: false,
        };
        (
            VoidType::get(ctx).to_handle(),
            false,
            RetKind::Void,
            None,
            Some(outcome),
        )
    } else {
        match lower_ty(name, ret_ty, layout, None)? {
            LowerTy::ZeroSized => (
                VoidType::get(ctx).to_handle(),
                false,
                RetKind::Void,
                None,
                None,
            ),
            LowerTy::Scalar(scalar) => (scalar.handle(ctx), true, scalar.ret_kind(), None, None),
            LowerTy::Aggregate { layout, .. } => (
                VoidType::get(ctx).to_handle(),
                false,
                RetKind::Void,
                Some(layout),
                None,
            ),
        }
    };
    let mut params = Vec::with_capacity(func.param_types.len());
    let mut param_handles = Vec::with_capacity(func.param_types.len() + 1);
    if sret.is_some() || outcome.is_some() {
        param_handles.push(PointerType::get(ctx, 0).into());
    }
    for (index, ty) in func.param_types.iter().enumerate() {
        let lowered = lower_ty(name, ty, layout, None)?;
        let by_reference = func.ref_params.get(index).copied().unwrap_or(false);
        match &lowered {
            // A `mut`/`ref` parameter passes as a pointer to the caller's
            // storage regardless of kind; the callee slot aliases it.
            _ if by_reference => param_handles.push(PointerType::get(ctx, 0).into()),
            LowerTy::Scalar(scalar) => param_handles.push(scalar.handle(ctx)),
            LowerTy::Aggregate { .. } => param_handles.push(PointerType::get(ctx, 0).into()),
            // A zero-sized parameter (a `NoneType` overload marker like
            // `__list_literal__`) carries no runtime data: it keeps its
            // signature entry but no physical argument.
            LowerTy::ZeroSized => {}
        }
        params.push(lowered);
    }
    let func_ty = FuncType::get(ctx, result, param_handles, false);
    let mangled = mangle::mangle(name);
    let identifier: Identifier = mangled
        .as_str()
        .try_into()
        .expect("mangled names are identifier-safe");
    let func_op = FuncOp::new(ctx, identifier, func_ty);
    module.append_operation(ctx, func_op.get_operation(), 0);
    Ok((
        func_op,
        FnSignature {
            mangled,
            func_ty,
            returns_value,
            params,
            ret,
            sret,
            outcome,
            owned_params: func.owned_params.clone(),
            ref_params: func.ref_params.clone(),
            deinit_receiver: func.deinit_params.first().copied().unwrap_or(false),
            empty_body: func.blocks.iter().all(|block| block.instrs.is_empty()),
        },
    ))
}

/// Synthesize the executable's C `main`: reference the linked runtime's
/// version entry point (`mjrt_version`, keeping the inspectable
/// `mjrt_abi_version` data symbol in every produced binary), call each
/// (void, zero-arg) callee in order, then return `0: i32`. Callees are
/// already-mangled native symbols.
pub(crate) fn synthesize_exe_wrapper(
    ctx: &mut Context,
    module: ModuleOp,
    callees: &[(String, Option<(Layout, u64)>)],
    unhandled_error_declared: bool,
) -> Result<(), PlironError> {
    let void = VoidType::get(ctx).to_handle();
    let i32_ty: TypeHandle = IntegerType::get(ctx, 32, Signedness::Signless).into();
    let i32_int = IntegerType::get(ctx, 32, Signedness::Signless);
    let i64_int = IntegerType::get(ctx, 64, Signedness::Signless);
    let i64_ty: TypeHandle = i64_int.into();
    let i8_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
    let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
    let version_ty = FuncType::get(ctx, i32_ty, vec![], false);
    let version = FuncOp::new(
        ctx,
        "mjrt_version".try_into().expect("valid identifier"),
        version_ty,
    );
    module.append_operation(ctx, version.get_operation(), 0);
    // A raising entry propagates its error out to the wrapper, which reports
    // it as the unhandled-error trap (stderr text and exit 69 unchanged from
    // an in-callee report).
    let unhandled_ty = FuncType::get(ctx, void, vec![ptr_ty, i64_ty], false);
    if callees.iter().any(|(_, outcome)| outcome.is_some()) && !unhandled_error_declared {
        let unhandled = FuncOp::new(
            ctx,
            "mjrt_unhandled_error".try_into().expect("valid identifier"),
            unhandled_ty,
        );
        module.append_operation(ctx, unhandled.get_operation(), 0);
    }
    let wrapper_ty = FuncType::get(ctx, i32_ty, vec![], false);
    let wrapper = FuncOp::new(
        ctx,
        "main".try_into().expect("valid identifier"),
        wrapper_ty,
    );
    module.append_operation(ctx, wrapper.get_operation(), 0);
    let entry = wrapper.get_or_create_entry_block(ctx);
    let region = wrapper
        .get_operation()
        .deref(ctx)
        .regions()
        .next()
        .expect("llvm.func has a body region");
    let mut current = entry;

    let version_call = CallOp::new(
        ctx,
        CallOpCallable::Direct("mjrt_version".try_into().expect("valid identifier")),
        version_ty,
        vec![],
    );
    version_call.get_operation().insert_at_back(current, ctx);
    for (callee, outcome) in callees {
        let identifier: Identifier = callee
            .as_str()
            .try_into()
            .expect("mangled names are identifier-safe");
        let Some((layout, err_offset)) = outcome else {
            let callee_ty = FuncType::get(ctx, void, vec![], false);
            let call = CallOp::new(ctx, CallOpCallable::Direct(identifier), callee_ty, vec![]);
            call.get_operation().insert_at_back(current, ctx);
            continue;
        };
        let count_attr = IntegerAttr::new(i64_int, APInt::from_u64(layout.size.max(1), bw(64)));
        let count = ConstantOp::new(ctx, Box::new(count_attr));
        count.get_operation().insert_at_back(current, ctx);
        let storage = AllocaOp::new(ctx, i8_ty, count.get_result(ctx));
        storage.set_alignment(ctx, layout.align as u32);
        storage.get_operation().insert_at_back(current, ctx);
        let callee_ty = FuncType::get(ctx, void, vec![ptr_ty], false);
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct(identifier),
            callee_ty,
            vec![storage.get_result(ctx)],
        );
        call.get_operation().insert_at_back(current, ctx);
        let tag = LoadOp::new(ctx, storage.get_result(ctx), i32_ty);
        tag.get_operation().insert_at_back(current, ctx);
        let err_attr = IntegerAttr::new(
            i32_int,
            APInt::from_u64(u64::from(mojito_native::native::rt_abi::MJ_TAG_ERR), bw(32)),
        );
        let err_tag = ConstantOp::new(ctx, Box::new(err_attr));
        err_tag.get_operation().insert_at_back(current, ctx);
        let is_err = ICmpOp::new(
            ctx,
            ICmpPredicateAttr::EQ,
            tag.get_result(ctx),
            err_tag.get_result(ctx),
        );
        is_err.get_operation().insert_at_back(current, ctx);
        let err_block = BasicBlock::new(ctx, None, vec![]);
        err_block.insert_at_back(region, ctx);
        let cont_block = BasicBlock::new(ctx, None, vec![]);
        cont_block.insert_at_back(region, ctx);
        let branch = CondBrOp::new(
            ctx,
            is_err.get_result(ctx),
            err_block,
            vec![],
            cont_block,
            vec![],
        );
        branch.get_operation().insert_at_back(current, ctx);
        // The error's message is the MjString at the error offset:
        // `{ data, size, cap }`.
        let data_address = if *err_offset == 0 {
            storage.get_result(ctx)
        } else {
            let index = u32::try_from(*err_offset).expect("outcome offsets fit u32");
            let gep = GetElementPtrOp::new(
                ctx,
                storage.get_result(ctx),
                vec![GepIndex::Constant(index)],
                i8_ty,
            );
            gep.get_operation().insert_at_back(err_block, ctx);
            gep.get_result(ctx)
        };
        let data = LoadOp::new(ctx, data_address, ptr_ty);
        data.get_operation().insert_at_back(err_block, ctx);
        let size_index = u32::try_from(*err_offset + 8).expect("outcome offsets fit u32");
        let size_gep = GetElementPtrOp::new(
            ctx,
            storage.get_result(ctx),
            vec![GepIndex::Constant(size_index)],
            i8_ty,
        );
        size_gep.get_operation().insert_at_back(err_block, ctx);
        let size = LoadOp::new(ctx, size_gep.get_result(ctx), i64_ty);
        size.get_operation().insert_at_back(err_block, ctx);
        let report = CallOp::new(
            ctx,
            CallOpCallable::Direct("mjrt_unhandled_error".try_into().expect("valid identifier")),
            unhandled_ty,
            vec![data.get_result(ctx), size.get_result(ctx)],
        );
        report.get_operation().insert_at_back(err_block, ctx);
        let unreachable = UnreachableOp::new(ctx);
        unreachable.get_operation().insert_at_back(err_block, ctx);
        current = cont_block;
    }
    let zero_attr = IntegerAttr::new(i32_int, APInt::from_u64(0, bw(32)));
    let zero = ConstantOp::new(ctx, Box::new(zero_attr));
    zero.get_operation().insert_at_back(current, ctx);
    let ret = ReturnOp::new(ctx, Some(zero.get_result(ctx)));
    ret.get_operation().insert_at_back(current, ctx);
    Ok(())
}

/// Calls the backend lowers as runtime-ABI intrinsics instead of compiling
/// their stdlib bodies: the `std.memory` allocation entry points allocate
/// through element-erased builtins (the VM's slot arena) whose element sizes
/// only exist at the specialized call sites the backend intercepts.
/// `reachable_set` skips these edges so the erased bodies are never declared.
pub(crate) fn intercepted_call(name: &str) -> bool {
    name == "__module$std$memory$unsafe_alloc"
        || name.starts_with("__module$std$memory$unsafe_alloc$")
}

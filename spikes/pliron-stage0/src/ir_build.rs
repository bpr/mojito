//! Programmatic construction of LLVM-dialect IR: the Stage 0 `main -> i32`
//! acceptance program and a deliberately invalid module for the negative
//! verification test.

use pliron::{
    builtin::{
        attributes::IntegerAttr,
        op_interfaces::{OneResultInterface, SingleBlockRegionInterface},
        ops::ModuleOp,
        types::{IntegerType, Signedness},
    },
    context::Context,
    op::{Op, verify_op},
    result::Result,
    utils::apint::{APInt, bw},
    value::Value,
};
use pliron_llvm::{
    attributes::IntegerOverflowFlagsAttr,
    op_interfaces::IntBinArithOpWithOverflowFlag,
    ops::{AddOp, ConstantOp, FuncOp, ReturnOp, SubOp},
    types::FuncType,
};

/// Build and verify `module { llvm.func @main() -> i32 { return 40 + 2 } }`.
pub fn build_main_returns_42(ctx: &mut Context) -> Result<ModuleOp> {
    let module = ModuleOp::new(ctx, "spike".try_into().unwrap());
    let (func, c40, c2) = build_i32_func(ctx, "main");
    module.append_operation(ctx, func.get_operation(), 0);

    let bb = func.get_entry_block(ctx).expect("entry block exists");
    let flags = IntegerOverflowFlagsAttr {
        nsw: false,
        nuw: false,
    };
    let sum = AddOp::new_with_overflow_flag(ctx, c40, c2, flags);
    sum.get_operation().insert_at_back(bb, ctx);
    let ret = ReturnOp::new(ctx, Some(sum.get_result(ctx)));
    ret.get_operation().insert_at_back(bb, ctx);

    verify_op(&module, ctx)?;
    Ok(module)
}

/// Like [build_main_returns_42], plus one dead `llvm.sub` of the two
/// constants. Feed for the pass test: the fold pass eliminates the add, and
/// built-in DCE eliminates the unused sub.
pub fn build_main_with_dead_sub(ctx: &mut Context) -> Result<ModuleOp> {
    let module = ModuleOp::new(ctx, "spike_dead".try_into().unwrap());
    let (func, c40, c2) = build_i32_func(ctx, "main");
    module.append_operation(ctx, func.get_operation(), 0);

    let bb = func.get_entry_block(ctx).expect("entry block exists");
    let flags = IntegerOverflowFlagsAttr {
        nsw: false,
        nuw: false,
    };
    let dead = SubOp::new_with_overflow_flag(ctx, c40, c2, flags.clone());
    dead.get_operation().insert_at_back(bb, ctx);
    let sum = AddOp::new_with_overflow_flag(ctx, c40, c2, flags);
    sum.get_operation().insert_at_back(bb, ctx);
    let ret = ReturnOp::new(ctx, Some(sum.get_result(ctx)));
    ret.get_operation().insert_at_back(bb, ctx);

    verify_op(&module, ctx)?;
    Ok(module)
}

/// Build a module whose `main` returns an i64 constant from an
/// `() -> i32` function: well-formed structurally, semantically invalid.
/// Verification must fail without panicking.
pub fn build_invalid_module(ctx: &mut Context) -> ModuleOp {
    let module = ModuleOp::new(ctx, "spike_invalid".try_into().unwrap());
    let (func, c40, _) = build_i32_func(ctx, "main");
    module.append_operation(ctx, func.get_operation(), 0);

    let bb = func.get_entry_block(ctx).expect("entry block exists");
    let i64_ty = IntegerType::get(ctx, 64, Signedness::Signless);
    let bad = ConstantOp::new(
        ctx,
        Box::new(IntegerAttr::new(i64_ty, APInt::from_u64(7, bw(64)))),
    );
    bad.get_operation().insert_at_back(bb, ctx);
    let flags = IntegerOverflowFlagsAttr {
        nsw: false,
        nuw: false,
    };
    // i32 + i64 operands: violates SameOperandsType on llvm.add.
    let sum = AddOp::new_with_overflow_flag(ctx, c40, bad.get_result(ctx), flags);
    sum.get_operation().insert_at_back(bb, ctx);
    let ret = ReturnOp::new(ctx, Some(sum.get_result(ctx)));
    ret.get_operation().insert_at_back(bb, ctx);

    module
}

/// Build an `() -> i32` LLVM-dialect function containing constants 40 and 2
/// in a fresh entry block; the caller appends the rest of the body.
fn build_i32_func(ctx: &mut Context, name: &str) -> (FuncOp, Value, Value) {
    let i32_ty = IntegerType::get(ctx, 32, Signedness::Signless);
    let func_ty = FuncType::get(ctx, i32_ty.into(), vec![], false);
    let func = FuncOp::new(ctx, name.try_into().unwrap(), func_ty);
    let bb = func.get_or_create_entry_block(ctx);

    let c40 = ConstantOp::new(
        ctx,
        Box::new(IntegerAttr::new(i32_ty, APInt::from_u64(40, bw(32)))),
    );
    c40.get_operation().insert_at_back(bb, ctx);
    let c2 = ConstantOp::new(
        ctx,
        Box::new(IntegerAttr::new(i32_ty, APInt::from_u64(2, bw(32)))),
    );
    c2.get_operation().insert_at_back(bb, ctx);

    (func, c40.get_result(ctx), c2.get_result(ctx))
}

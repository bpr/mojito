//! Stage 0 facility: a custom dialect op with textual syntax and a verifier
//! (positive and negative), lowered to the LLVM dialect through pliron's
//! dialect-conversion framework, with a legality walk and end-to-end
//! execution of the converted module.

use expect_test::expect;
use pliron::{
    builtin::{
        op_interfaces::{OneResultInterface, SingleBlockRegionInterface},
        ops::ModuleOp,
        types::{IntegerType, Signedness},
    },
    context::Context,
    op::{Op, verify_op},
    operation::verify_operation,
    pass::{AnalysisManager, Pass},
    printable::Printable,
    result::ExpectOk,
};
use pliron_llvm::{
    llvm_sys::{core::LLVMContext, lljit::LLVMLLJIT, target::initialize_native},
    ops::{FuncOp, ReturnOp},
    to_llvm_ir,
    types::FuncType,
};
use pliron_stage0_spike::{
    canonical_text, parse_top_level, print_ir,
    spike_dialect::{AnswerOp, count_ops_in_dialect, spike_to_llvm_pass},
};

/// Build `module { llvm.func @main() -> i32 { a = spike.answer; return a } }`.
fn build_spike_module(ctx: &mut Context) -> ModuleOp {
    let module = ModuleOp::new(ctx, "spike_mixed".try_into().unwrap());
    let i32_ty = IntegerType::get(ctx, 32, Signedness::Signless);
    let func_ty = FuncType::get(ctx, i32_ty.into(), vec![], false);
    let func = FuncOp::new(ctx, "main".try_into().unwrap(), func_ty);
    module.append_operation(ctx, func.get_operation(), 0);
    let bb = func.get_or_create_entry_block(ctx);
    let answer = AnswerOp::new(ctx);
    answer.get_operation().insert_at_back(bb, ctx);
    let ret = ReturnOp::new(ctx, Some(answer.get_result(ctx)));
    ret.get_operation().insert_at_back(bb, ctx);
    module
}

#[test]
fn spike_op_verifies_prints_and_reparses() {
    let ctx = &mut Context::new();
    let module = build_spike_module(ctx);
    verify_op(&module, ctx).expect_ok(ctx);

    let canonical = canonical_text(ctx, module.get_operation());
    assert!(canonical.contains("spike.answer"), "{canonical}");

    // As pinned in t02: the first parse attaches locations, so byte
    // stability is asserted from the first reparse onward.
    let ctx1 = &mut Context::new();
    let round1 = parse_top_level(ctx1, &canonical).expect_ok(ctx1);
    verify_operation(round1, ctx1).expect_ok(ctx1);
    let round1_text = canonical_text(ctx1, round1);

    let ctx2 = &mut Context::new();
    let round2 = parse_top_level(ctx2, &round1_text).expect_ok(ctx2);
    verify_operation(round2, ctx2).expect_ok(ctx2);
    assert_eq!(round1_text, canonical_text(ctx2, round2));
}

#[test]
fn spike_verifier_rejects_non_i32_result() {
    let ctx = &mut Context::new();
    let i64_ty = IntegerType::get(ctx, 64, Signedness::Signless);
    let bad = AnswerOp::new_with_result_type(ctx, i64_ty.into());
    let err = verify_op(&bad, ctx).expect_err("i64 spike.answer must be rejected");
    let message = err.disp(ctx).to_string();
    assert!(
        message.contains("signless i32"),
        "unexpected diagnostic: {message}"
    );
}

#[test]
fn lowering_leaves_no_spike_ops_and_executes() {
    let ctx = &mut Context::new();
    let module = build_spike_module(ctx);
    verify_op(&module, ctx).expect_ok(ctx);
    assert_eq!(
        count_ops_in_dialect(ctx, module.get_operation(), "spike"),
        1
    );

    spike_to_llvm_pass()
        .run(module.get_operation(), ctx, &mut AnalysisManager::default())
        .expect_ok(ctx);

    // Conversion legality: zero residual spike ops, and the module verifies.
    assert_eq!(
        count_ops_in_dialect(ctx, module.get_operation(), "spike"),
        0,
        "conversion must be total:\n{}",
        print_ir(ctx, module.get_operation())
    );
    verify_op(&module, ctx).expect_ok(ctx);

    let printed = print_ir(ctx, module.get_operation());
    expect![[r#"
        builtin.module @spike_mixed 
        {
          ^block1v1():
            llvm.func @main: llvm.func <builtin.integer i32() variadic = false>
              [] 
            {
              ^entry_block2v1():
                v1 = llvm.constant <builtin.integer <42: i32>> : builtin.integer i32;
                llvm.return v1
            }
        }"#]]
    .assert_eq(&printed);

    // The lowered module executes and returns the answer.
    initialize_native().expect("native target initialization");
    let llvm_ctx = LLVMContext::default();
    let llvm_module = to_llvm_ir::convert_module(ctx, &llvm_ctx, module).expect_ok(ctx);
    llvm_module.verify().expect("LLVM module verifies");
    let jit = LLVMLLJIT::new_with_default_builder().expect("LLJIT builder");
    jit.add_module(llvm_module).expect("add module to JIT");
    let addr = jit.lookup_symbol("main").expect("main symbol resolves");
    let main_fn = unsafe { std::mem::transmute::<u64, fn() -> i32>(addr) };
    assert_eq!(main_fn(), 42);
}

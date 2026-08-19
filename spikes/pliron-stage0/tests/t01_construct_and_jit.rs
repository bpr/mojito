//! Stage 0 facility: programmatic IR construction, verification, LLVM module
//! conversion, and in-process host execution (ORC LLJIT) of `main -> i32`.

use pliron::{context::Context, result::ExpectOk};
use pliron_llvm::{
    llvm_sys::{core::LLVMContext, lljit::LLVMLLJIT, target::initialize_native},
    to_llvm_ir,
};
use pliron_stage0_spike::ir_build::build_main_returns_42;

#[test]
fn builds_verifies_and_jit_executes_main_returning_42() {
    let ctx = &mut Context::new();
    let module = build_main_returns_42(ctx).expect_ok(ctx);

    initialize_native().expect("native target initialization");
    let llvm_ctx = LLVMContext::default();
    let llvm_module = to_llvm_ir::convert_module(ctx, &llvm_ctx, module).expect_ok(ctx);
    llvm_module.verify().expect("LLVM module verifies");

    let jit = LLVMLLJIT::new_with_default_builder().expect("LLJIT builder");
    jit.add_module(llvm_module).expect("add module to JIT");
    let addr = jit.lookup_symbol("main").expect("main symbol resolves");
    assert_ne!(addr, 0);
    let main_fn = unsafe { std::mem::transmute::<u64, fn() -> i32>(addr) };
    assert_eq!(main_fn(), 42);

    // The pliron module was consumed by conversion only logically; the pliron
    // IR itself must still verify after export.
    pliron::op::verify_op(&module, ctx).expect_ok(ctx);
}

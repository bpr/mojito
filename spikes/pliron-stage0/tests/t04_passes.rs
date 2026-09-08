//! Stage 0 facility: a hand-written rewriting pass (constant folding) plus a
//! built-in pass (DCE) composed through the pass manager, with the
//! AnalysisManager driving invalidation between them.
//!
//! Finding (recorded in docs/notes/pliron-stage0.md): `llvm.constant` is
//! missing from pliron-llvm 0.17.0's `SideEffects`-false list
//! (`interface_impls.rs`), so built-in DCE conservatively keeps dead
//! constants. DCE does remove the dead `llvm.sub`, which has the interface.
//! Both behaviors are pinned below.

use expect_test::expect;
use pliron::{
    context::Context,
    op::Op,
    opts::dce::DCEPass,
    pass::{AnalysisManager, NestedOpsPass, OpPass, Pass, Passes},
    result::ExpectOk,
};
use pliron_llvm::ops::FuncOp;
use pliron_stage0_spike::{const_fold::FoldConstAdd, ir_build::build_main_with_dead_sub, print_ir};

#[test]
fn constfold_folds_add_and_dce_removes_dead_sub() {
    let ctx = &mut Context::new();
    let module = build_main_with_dead_sub(ctx).expect_ok(ctx);

    let mut pipeline = Passes::default();
    pipeline.add_pass(FoldConstAdd);
    let mut per_func = Passes::default();
    per_func.add_pass(OpPass::<FuncOp, DCEPass>::default());
    pipeline.add_pass(NestedOpsPass::new(per_func));

    pipeline
        .run(module.get_operation(), ctx, &mut AnalysisManager::default())
        .expect_ok(ctx);

    pliron::op::verify_op(&module, ctx).expect_ok(ctx);
    let printed = print_ir(ctx, module.get_operation());
    expect![[r#"
        builtin.module @spike_dead 
        {
          ^block1v1():
            llvm.func @main: llvm.func <builtin.integer i32() variadic = false>
              [] 
            {
              ^entry_block2v1():
                v4 = llvm.constant <builtin.integer <42: i32>> : builtin.integer i32;
                llvm.return v4
            }
        }"#]]
    .assert_eq(&printed);

    assert!(
        !printed.contains("llvm.add"),
        "the add must be folded away:\n{printed}"
    );
    assert!(
        !printed.contains("llvm.sub"),
        "DCE must remove the unused sub:\n{printed}"
    );
    // DCE removes the two inputs after the add is replaced by folded 42.
    assert_eq!(
        printed.matches("llvm.constant").count(),
        1,
        "expected only the folded constant to remain:\n{printed}"
    );
}

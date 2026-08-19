//! Stage 0 facility: LLVM IR text export, bitcode export, object emission and
//! linking through clang, and execution of the produced host executable
//! (`main -> i32` exits with 42).

use std::process::Command;

use expect_test::expect;
use pliron::{context::Context, result::ExpectOk};
use pliron_llvm::{llvm_sys::core::LLVMContext, to_llvm_ir};
use pliron_stage0_spike::ir_build::build_main_returns_42;

/// Prefer the version-suffixed clang matching llvm-sys 221; fall back to the
/// unsuffixed binary (this machine's default clang is LLVM 22 as well).
fn find_clang() -> &'static str {
    for candidate in ["clang-22", "clang"] {
        if Command::new(candidate)
            .arg("--version")
            .output()
            .is_ok_and(|out| out.status.success())
        {
            return candidate;
        }
    }
    panic!("no clang found; install clang (LLVM 22) to run this test");
}

#[test]
fn textual_ll_bitcode_and_native_executable_exit_42() {
    let ctx = &mut Context::new();
    let module = build_main_returns_42(ctx).expect_ok(ctx);

    let llvm_ctx = LLVMContext::default();
    let llvm_module = to_llvm_ir::convert_module(ctx, &llvm_ctx, module).expect_ok(ctx);
    llvm_module.verify().expect("LLVM module verifies");

    expect![[r#"
        ; ModuleID = 'spike'
        source_filename = "spike"

        define i32 @main() {
        entry_block2v1:
          ret i32 42
        }
    "#]]
    .assert_eq(&llvm_module.to_string());

    let tmp = tempfile::tempdir().expect("tempdir");
    let bc_path = tmp.path().join("main.bc");
    let exe_path = tmp.path().join("main");
    llvm_module
        .bitcode_to_file(bc_path.to_str().unwrap())
        .expect("bitcode export");

    let clang = Command::new(find_clang())
        .arg(&bc_path)
        .arg("-o")
        .arg(&exe_path)
        .output()
        .expect("clang runs");
    assert!(
        clang.status.success(),
        "clang failed: {}",
        String::from_utf8_lossy(&clang.stderr)
    );

    let run = Command::new(&exe_path).status().expect("executable runs");
    assert_eq!(run.code(), Some(42), "main must exit with 42");
}

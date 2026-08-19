//! Stage 0 facility: invalid IR must produce a `Result` diagnostic (never a
//! panic), and diagnostics on parsed IR must carry a source location.

use pliron::{
    context::Context, op::verify_op, operation::verify_operation, printable::Printable,
    result::ExpectOk,
};
use pliron_stage0_spike::{ir_build::build_invalid_module, parse_top_level};

/// Well-formed but semantically invalid: `llvm.add` over i32 + i64.
const INVALID_ADD_MODULE: &str = r#"
    builtin.module @m {
    ^entry():
      llvm.func @main: llvm.func <builtin.integer i32 () variadic = false> [] {
        ^entry():
        a = llvm.constant <builtin.integer <40: i32>> : builtin.integer i32;
        b = llvm.constant <builtin.integer <7: i64>> : builtin.integer i64;
        sum = llvm.add a, b <{nsw=false,nuw=false}> : builtin.integer i32;
        llvm.return sum
      }
    }
"#;

#[test]
fn constructed_invalid_ir_errors_without_panicking() {
    let outcome = std::panic::catch_unwind(|| {
        let ctx = &mut Context::new();
        let module = build_invalid_module(ctx);
        verify_op(&module, ctx).map_err(|err| err.disp(ctx).to_string())
    });
    let verified = outcome.expect("verification must not panic on invalid IR");
    let message = verified.expect_err("mixed-width llvm.add must fail verification");
    assert!(
        message.contains("Compilation error"),
        "diagnostic should render as a compilation error, got: {message}"
    );
}

#[test]
fn parsed_invalid_ir_reports_located_diagnostic() {
    let ctx = &mut Context::new();
    let op = parse_top_level(ctx, INVALID_ADD_MODULE).expect_ok(ctx);
    let err = verify_operation(op, ctx).expect_err("verification must fail");
    let message = err.disp(ctx).to_string();
    assert!(
        message.contains("Compilation error"),
        "unexpected rendering: {message}"
    );
    assert!(
        message.contains("line") || message.contains(':'),
        "diagnostic should carry a source location, got: {message}"
    );
}

#[test]
fn malformed_text_reports_located_parse_error() {
    let ctx = &mut Context::new();
    let err = parse_top_level(ctx, "builtin.module @broken { this is not IR }")
        .expect_err("malformed text must fail to parse");
    let message = err.disp(ctx).to_string();
    assert!(
        message.contains("line") && message.contains("column"),
        "parse error should carry line/column, got: {message}"
    );
}

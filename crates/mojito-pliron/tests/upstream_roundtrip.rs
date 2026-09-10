//! Upstream contract: canonical textual printing and parse -> reprint round
//! trips, which `compile` relies on for its canonical Pliron text cache.

mod support;

use expect_test::expect;
use pliron::{context::Context, op::Op, operation::verify_operation, result::ExpectOk};
use support::{canonical_text, ir_build::build_main_returns_42, parse_top_level, print_ir};

#[test]
fn constructed_ir_print_snapshot() {
    let ctx = &mut Context::new();
    let module = build_main_returns_42(ctx).expect_ok(ctx);
    let printed = print_ir(ctx, module.get_operation());

    expect![[r"
        builtin.module @spike 
        {
          ^block1v1():
            llvm.func @main: llvm.func <builtin.integer i32() variadic = false>
              [] 
            {
              ^entry_block2v1():
                v0 = llvm.constant <builtin.integer <40: i32>> : builtin.integer i32;
                v1 = llvm.constant <builtin.integer <2: i32>> : builtin.integer i32;
                v2 = llvm.add v0, v1 <{nsw=false,nuw=false}>: builtin.integer i32;
                llvm.return v2
            }
        }"]]
    .assert_eq(&printed);
}

#[test]
fn canonical_parse_print_is_byte_stable() {
    let ctx = &mut Context::new();
    let module = build_main_returns_42(ctx).expect_ok(ctx);
    let canonical = canonical_text(ctx, module.get_operation());

    // Two full parse -> canonicalize -> print rounds, each in a fresh
    // context: the text must be byte-identical throughout.
    let ctx1 = &mut Context::new();
    let round1 = parse_top_level(ctx1, &canonical).expect_ok(ctx1);
    verify_operation(round1, ctx1).expect_ok(ctx1);
    let round1_text = canonical_text(ctx1, round1);

    let ctx2 = &mut Context::new();
    let round2 = parse_top_level(ctx2, &round1_text).expect_ok(ctx2);
    verify_operation(round2, ctx2).expect_ok(ctx2);
    let round2_text = canonical_text(ctx2, round2);

    assert_eq!(
        round1_text, round2_text,
        "canonical parse -> print must be byte-stable"
    );
}

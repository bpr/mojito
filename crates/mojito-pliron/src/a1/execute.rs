//! Execution of a parsed core module through the existing backends.
//!
//! Core is exported once to verified MIR, and that one program feeds both
//! the VM and the native compiler. Nothing here reads the importer's input.

use pliron::context::Context;
use pliron::irfmt::parsers::spaced;
use pliron::operation::Operation;
use pliron::parsable::parse_from_str;
use pliron::printable::Printable;

use super::export::{Exported, export_program};
use super::inventory::Stage;
use super::outcomes::denormalize;
use super::text::parse_text;
use super::verify::{verify_module, walk};
use super::{A1Error, A1ErrorKind};
use crate::NativeModule;

/// Parse executable-core text in a fresh context and export it to
/// verified MIR.
pub fn program_of_text(text: &str) -> Result<Exported, A1Error> {
    let mut parsed = parse_text(text, Stage::ExecutableCore)?;
    denormalize(&mut parsed.ctx, parsed.module)?;
    verify_module(&parsed.ctx, parsed.module, Stage::Bridge)?;
    let exported = export_program(&parsed.ctx, parsed.module)?;
    let findings = mojito_mir::mir::verify::verify(&exported.program);
    if findings.is_empty() {
        return Ok(exported);
    }
    Err(A1Error::new(
        A1ErrorKind::Export,
        format!("the exported MIR does not verify: {}", findings.join("; ")),
    ))
}

/// The legality of a compiled native module before LLVM export: only the
/// builtin container and LLVM-dialect operations remain.
pub fn native_legality(module: &NativeModule) -> Result<(), A1Error> {
    let mut ctx = Context::new();
    let parsed = parse_from_str(
        spaced(Operation::top_level_parser()),
        &mut ctx,
        module.plir_text(),
    )
    .map_err(|error| A1Error::new(A1ErrorKind::Parse, error.disp(&ctx).to_string()))?;
    for op in walk(&ctx, parsed) {
        let opid = Operation::get_opid(op, &ctx);
        let dialect = opid.dialect.to_string();
        if dialect != "llvm" && dialect != "builtin" {
            return Err(A1Error::new(
                A1ErrorKind::Legality,
                format!("`{opid}` remains in the native module"),
            ));
        }
    }
    Ok(())
}

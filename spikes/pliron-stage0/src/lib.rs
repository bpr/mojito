//! Pliron Stage 0 feasibility spike (roadmap section 4, Stage 0).
//!
//! Validates pliron + pliron-llvm 0.17.0 against LLVM 22 outside the
//! production compiler: IR construction, textual round-trips, verification
//! failure with located diagnostics, passes, a toy-dialect lowering through
//! the dialect-conversion framework, LLVM export, and host execution.
//! Nothing here is reachable from `mojito`.

pub mod const_fold;
pub mod ir_build;
pub mod spike_dialect;

use pliron::{
    context::{Context, Ptr},
    irfmt::parsers::spaced,
    operation::Operation,
    parsable::parse_from_str,
    printable::Printable,
    result::Result,
};

/// Parse a top-level operation from PLIR text.
pub fn parse_top_level(ctx: &mut Context, input: &str) -> Result<Ptr<Operation>> {
    parse_from_str(spaced(Operation::top_level_parser()), ctx, input)
}

/// Print any IR entity registered with the context to a `String`.
pub fn print_ir(ctx: &Context, op: Ptr<Operation>) -> String {
    op.disp(ctx).to_string()
}

/// Erase user-given SSA/block names, then print.
///
/// Plain `parse -> print` is not a fixpoint in pliron 0.17.0: the parser
/// stores each parsed block label as a given name and the printer re-suffixes
/// it with the internal id, so block labels grow on every round trip.
/// Erasing given names first makes the printed text byte-stable.
pub fn canonical_text(ctx: &mut Context, op: Ptr<Operation>) -> String {
    pliron::debug_info::erase_given_names(ctx, op);
    op.disp(ctx).to_string()
}

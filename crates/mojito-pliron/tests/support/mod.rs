//! Upstream-contract helpers shared by the `upstream_*` integration tests.
//!
//! These tests pin the Pliron/`pliron-llvm` API surface this crate depends on,
//! so a revision bump that changes upstream behaviour fails here — against a
//! small hand-built module — rather than deep inside MIR lowering.

pub mod const_fold;
pub mod ir_build;

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
/// Plain `parse -> print` is not a fixpoint: the parser stores each parsed
/// block label as a given name and the printer re-suffixes it with the
/// internal id, so block labels grow on every round trip. Erasing given names
/// first makes the printed text byte-stable.
pub fn canonical_text(ctx: &mut Context, op: Ptr<Operation>) -> String {
    pliron::builtin::given_names::erase_given_names(ctx, op);
    op.disp(ctx).to_string()
}

//! The adapter over the pinned Pliron: context creation, printing,
//! parsing, and verification. Upstream API drift lands here.

use pliron::context::{Context, Ptr};
use pliron::irfmt::parsers::spaced;
use pliron::operation::Operation;
use pliron::parsable::parse_from_str;
use pliron::printable::Printable;

use super::inventory::Stage;
use super::verify::verify_module;
use super::{A1Error, A1ErrorKind};

/// A fresh context with every linked dialect registered.
pub fn new_context() -> Context {
    Context::new()
}

/// The module as Pliron text, exactly as the context holds it.
pub fn print(ctx: &Context, module: Ptr<Operation>) -> String {
    module.disp(ctx).to_string()
}

/// Parse a module from text into `ctx` and verify it at `stage`.
pub fn parse(ctx: &mut Context, text: &str, stage: Stage) -> Result<Ptr<Operation>, A1Error> {
    let module = parse_unverified(ctx, text)?;
    verify_module(ctx, module, stage)?;
    Ok(module)
}

/// Parse a module from text into `ctx`, leaving verification to the caller.
pub fn parse_unverified(ctx: &mut Context, text: &str) -> Result<Ptr<Operation>, A1Error> {
    parse_from_str(spaced(Operation::top_level_parser()), ctx, text)
        .map_err(|error| A1Error::new(A1ErrorKind::Parse, error.disp(ctx).to_string()))
}

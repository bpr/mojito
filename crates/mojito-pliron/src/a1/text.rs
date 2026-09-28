//! Canonical text: one spelling per module, stable from the first print.
//!
//! Pliron names values and blocks by arena index, so text depends on
//! allocation history. The canonical spelling is therefore printed from a
//! fresh context the module was parsed into: the parser allocates in the
//! order of the text's structure, which no name and no history affects.

use pliron::builtin::given_names::erase_given_names;
use pliron::context::{Context, Ptr};
use pliron::operation::Operation;

use super::inventory::Stage;
use super::ir_framework::{new_context, parse, print};
use super::provenance::stamp_locations;
use super::{A1Error, A1ErrorKind};

/// The wrapper's first line. This is not `.mir`, and `exec` never reads it.
pub const HEADER: &str = "mojito-a1-core 0";

/// A module parsed from canonical text, with the context that owns it.
pub struct ParsedModule {
    pub ctx: Context,
    pub module: Ptr<Operation>,
}

/// The canonical text of `module`: UTF-8, LF, one trailing newline.
pub fn canonical_text(
    ctx: &mut Context,
    module: Ptr<Operation>,
    stage: Stage,
) -> Result<String, A1Error> {
    stamp_locations(ctx, module)?;
    erase_given_names(ctx, module);
    let first = print(ctx, module);
    let mut fresh = new_context();
    let reparsed = parse(&mut fresh, &first, stage)?;
    erase_given_names(&mut fresh, reparsed);
    Ok(wrap(&print(&fresh, reparsed)))
}

/// Parse canonical text into a fresh context and verify it at `stage`.
pub fn parse_text(text: &str, stage: Stage) -> Result<ParsedModule, A1Error> {
    let body = text
        .strip_prefix(HEADER)
        .and_then(|rest| rest.strip_prefix('\n'))
        .ok_or_else(|| {
            A1Error::new(
                A1ErrorKind::Parse,
                format!("the text does not begin with `{HEADER}`"),
            )
        })?;
    let mut ctx = new_context();
    let module = parse(&mut ctx, body, stage)?;
    Ok(ParsedModule { ctx, module })
}

fn wrap(body: &str) -> String {
    format!("{HEADER}\n{}\n", body.trim_end_matches('\n'))
}

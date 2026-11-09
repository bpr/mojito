//! The resolution of upstream's `kgen.source_loc` ([`MirInstr::SourceLoc`])
//! to the call site its `inlineCount` names, once the post-elaboration
//! inliner has built every op's call-site chain: the `(line, column,
//! file)` constants that site denotes, as upstream folds the op after
//! inlining.

use super::{Const, MirBlock, MirFunction, MirInstr, Reg, fresh_register, instruction_regions_mut};
use mojito_common::token::SourceSpan;
use mojito_types::ct::CtValue;
use mojito_types::types::{TyKind, tuple_elements};

/// The file a site in a compile-time application reports, upstream's
/// answer in a parameter context, beside its zero line and column.
pub const PARAMETER_CONTEXT_FILE: &str = "<unknown location in parameter context>";

/// The file a site no source text places reports, beside its zero line
/// and column.
pub const UNKNOWN_LOCATION_FILE: &str = "<unknown location>";

/// Where a site reports: its 1-based line and byte column, and its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePoint {
    pub line: i64,
    pub column: i64,
    pub file: String,
}

impl SourcePoint {
    /// The point of a site no source text places, at
    /// [`UNKNOWN_LOCATION_FILE`].
    pub fn unknown() -> Self {
        Self {
            line: 0,
            column: 0,
            file: UNKNOWN_LOCATION_FILE.to_string(),
        }
    }

    /// The `(line, column, file)` tuple a `kgen.source_loc` holds.
    pub fn value(&self) -> CtValue {
        CtValue::Tuple(vec![
            CtValue::Index(self.line),
            CtValue::Index(self.column),
            CtValue::KgenString(self.file.clone()),
        ])
    }
}

/// What a `kgen.source_loc` evaluated in a compile-time application holds:
/// the pin's `<unknown location in parameter context>:0:0`.
pub fn unknown_source_location() -> CtValue {
    SourcePoint {
        line: 0,
        column: 0,
        file: PARAMETER_CONTEXT_FILE.to_string(),
    }
    .value()
}

/// Fold every `kgen.source_loc` of `function` to the point of its site;
/// returns whether it folded one.
///
/// The site is the one `inline_count` frames out along the op's call-site
/// chain, the walk stopping at the outermost, as upstream's does. `locate`
/// places it; a site it cannot place reports [`SourcePoint::unknown`].
pub fn resolve_source_locations(
    function: &mut MirFunction,
    locate: &dyn Fn(&SourceSpan) -> Option<SourcePoint>,
) -> bool {
    let mut blocks = std::mem::take(&mut function.blocks);
    let changed = resolve_blocks(&mut blocks, function, locate);
    function.blocks = blocks;
    changed
}

fn resolve_blocks(
    blocks: &mut [MirBlock],
    function: &mut MirFunction,
    locate: &dyn Fn(&SourceSpan) -> Option<SourcePoint>,
) -> bool {
    let mut changed = false;
    for block in blocks {
        let mut index = 0;
        while index < block.instrs.len() {
            for region in instruction_regions_mut(&mut block.instrs[index]) {
                changed |= resolve_blocks(region, function, locate);
            }
            let MirInstr::SourceLoc {
                dest,
                inline_count,
                location,
            } = &block.instrs[index]
            else {
                index += 1;
                continue;
            };
            let dest = *dest;
            let site = resolved_site(function, dest, inline_count, location);
            let point = site.and_then(locate).unwrap_or_else(SourcePoint::unknown);
            let replacement = constant_tuple(function, dest, &point);
            let length = replacement.len();
            block.instrs.splice(index..=index, replacement);
            index += length;
            changed = true;
        }
    }
    changed
}

/// The site `inline_count` frames out along the op's chain: its
/// `location`, or its register's span when it carries none.
fn resolved_site<'f>(
    function: &'f MirFunction,
    dest: Reg,
    inline_count: &Const,
    location: &'f [SourceSpan],
) -> Option<&'f SourceSpan> {
    let frames = match inline_count {
        Const::Value(CtValue::Index(count) | CtValue::Int(count)) => {
            usize::try_from(*count).unwrap_or(0)
        }
        Const::Value(CtValue::UInt(count)) => usize::try_from(*count).unwrap_or(usize::MAX),
        Const::Int(count) => usize::try_from(*count).unwrap_or(0),
        _ => 0,
    };
    if location.is_empty() {
        return function.spans.0.get(&dest.0).map(|(span, _)| span);
    }
    location.get(frames.min(location.len() - 1))
}

/// The instructions that build `dest`'s `Tuple` from the line, column,
/// and file constants: each element in a fresh register of `dest`'s
/// element type, gathered into its `storage` field.
fn constant_tuple(function: &mut MirFunction, dest: Reg, point: &SourcePoint) -> Vec<MirInstr> {
    let span = function.spans.0.get(&dest.0).map(|(span, _)| span.clone());
    let elements = function
        .reg_types
        .get(&dest.0)
        .and_then(|ty| tuple_elements(ty.kind()))
        .map(|elements| elements.into_iter().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let CtValue::Tuple(values) = point.value() else {
        unreachable!("a source location is a tuple")
    };
    if elements.len() != values.len() {
        return vec![MirInstr::Const {
            dest,
            k: Const::Value(CtValue::Tuple(values)),
        }];
    }
    let mut instructions = Vec::with_capacity(5);
    let mut items = Vec::with_capacity(3);
    for (ty, value) in elements.iter().cloned().zip(values) {
        let value = match value {
            CtValue::Index(number) if ty.kind().is_int() => CtValue::Int(number),
            value => value,
        };
        let register = fresh_register(function, ty, span.as_ref());
        instructions.push(MirInstr::Const {
            dest: register,
            k: Const::Value(value),
        });
        items.push(register);
    }
    let storage = fresh_register(
        function,
        TyKind::Tuple(elements.clone()).into(),
        span.as_ref(),
    );
    instructions.push(MirInstr::MakeTuple {
        dest: storage,
        elems: items,
        element_types: Some(elements),
    });
    instructions.push(MirInstr::MakeStruct {
        dest,
        fields: vec![storage],
    });
    instructions
}

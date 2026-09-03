//! Stable vocabulary for Mojito textual MIR schema version 1.
//!
//! This module owns canonical spelling, disassembly, parsing, and verified
//! artifact loading. Artifact syntax may not derive from Rust `Debug` output,
//! and semantic verification policy stays in `crate::mir::verify` — this
//! module only maps its findings to artifact source locations.

use super::{Const, MirInstr, MirIntrinsicSubscript, MirProgram, MirTerm, Proj, UseMode};
use mojito_common::token::Span;
use mojito_types::types::Ty;
use std::collections::BTreeMap;
use std::fmt;

/// A successfully decoded MIR artifact and its assembly-source locations.
#[derive(Debug)]
pub struct ParsedArtifact {
    pub program: MirProgram,
    pub source_map: ArtifactSourceMap,
}

/// Byte spans in the assembly source, keyed by stable artifact paths.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactSourceMap {
    entries: BTreeMap<String, Span>,
}

impl ArtifactSourceMap {
    pub fn span(&self, path: &str) -> Option<Span> {
        self.entries.get(path).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, Span)> {
        self.entries
            .iter()
            .map(|(path, span)| (path.as_str(), *span))
    }
}

/// One source-located artifact syntax or structural error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactDiagnostic {
    pub span: Span,
    pub message: String,
    pub context: Vec<String>,
}

/// All recoverable diagnostics produced while parsing one artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactReport {
    pub source_name: String,
    pub diagnostics: Vec<ArtifactDiagnostic>,
}

impl fmt::Display for ArtifactReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} artifact error(s) in {}",
            self.diagnostics.len(),
            self.source_name
        )
    }
}

impl std::error::Error for ArtifactReport {}

/// Parse a Mojito MIR artifact without running semantic MIR verification.
/// [`load_artifact`] composes this decoding with the canonical verifier.
pub fn parse_artifact(
    input: &[u8],
    source_name: impl Into<String>,
) -> Result<ParsedArtifact, ArtifactReport> {
    parse::artifact(input, source_name.into())
}

/// Parse and semantically verify a Mojito MIR artifact — the loading gate
/// artifact execution sits behind.
pub fn load_artifact(
    input: &[u8],
    source_name: impl Into<String>,
) -> Result<ParsedArtifact, ArtifactReport> {
    let source_name = source_name.into();
    let parsed = parse_artifact(input, source_name.clone())?;
    verify_artifact(&parsed, source_name)?;
    Ok(parsed)
}

/// Run the canonical MIR semantic verifier on a parsed artifact, reporting
/// every finding at its artifact source location.
///
/// Verification policy stays in `crate::mir::verify`; this entry maps each
/// finding to the most precise assembly span the artifact's source map
/// records (block, then function, then the artifact root) and names the
/// resolved artifact path in the diagnostic context.
pub fn verify_artifact(
    parsed: &ParsedArtifact,
    source_name: impl Into<String>,
) -> Result<(), ArtifactReport> {
    let mut findings = parsed.program.invariant_errors.clone();
    findings.extend(super::verify::verify(&parsed.program));
    if findings.is_empty() {
        return Ok(());
    }
    let diagnostics = findings
        .into_iter()
        .map(|message| {
            let candidates = finding_candidates(
                parsed
                    .program
                    .functions
                    .iter()
                    .map(|(name, _)| name.as_str()),
                &message,
            );
            let (path, span) = resolve_candidate_span(&parsed.source_map, &candidates);
            ArtifactDiagnostic {
                span,
                message,
                context: vec![path],
            }
        })
        .collect();
    Err(ArtifactReport {
        source_name: source_name.into(),
        diagnostics,
    })
}

/// A verified-MIR program could not be represented by schema version 1.0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisassembleError {
    findings: Vec<String>,
}

impl DisassembleError {
    pub fn findings(&self) -> &[String] {
        &self.findings
    }
}

impl fmt::Display for DisassembleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cannot disassemble MIR: {}",
            self.findings.join("; ")
        )
    }
}

impl std::error::Error for DisassembleError {}

/// Print a verified MIR program in canonical Mojito MIR 1.0 text format.
///
/// Verification happens before emission, so an error never exposes partial
/// artifact text.
pub fn disassemble(program: &MirProgram) -> Result<String, DisassembleError> {
    let mut findings = program.invariant_errors.clone();
    findings.extend(super::verify::verify(program));
    findings.extend(write::schema_findings(program));
    findings.sort();
    findings.dedup();
    if !findings.is_empty() {
        return Err(DisassembleError { findings });
    }
    Ok(write::program(program))
}

pub const MAGIC: &str = "mojito-mir";
pub const VERSION_MAJOR: u16 = 1;
pub const VERSION_MINOR: u16 = 0;

pub const INSTRUCTION_MNEMONICS: &[&str] = &[
    "loans.establish",
    "interiors.invalidate",
    "ref.make",
    "ref.read",
    "value.copy",
    "ref.write",
    "closure.make",
    "lifetime.keep_alive",
    "const",
    "literal.materialize",
    "var.use",
    "place.move",
    "var.store",
    "unary",
    "binary",
    "call",
    "call.indirect",
    "call.method",
    "pointer.take",
    "pointer.destroy",
    "uninit.make",
    "uninit.take",
    "uninit.destroy",
    "field.get",
    "index.get",
    "slice.get",
    "index.multi",
    "index.multi_set",
    "place.store",
    "place.store_ref",
    "place.load",
    "tuple.make",
    "variant.make",
    "variant.is",
    "variant.get",
    "variant.set",
    "variant.take",
    "variant.set_init_with",
    "variant.deinit_with",
    "variant.replace",
    "simd.make",
    "simd.cast",
    "simd.shuffle",
    "raise",
    "try",
    "drop.reg",
    "drop.var",
    "consume.var",
    "consume.place",
    "unsupported",
    "iter.init",
    "iter.has_next",
    "iter.next",
    "iter.try_next",
];

pub const TERMINATOR_MNEMONICS: &[&str] = &[
    "jump",
    "branch",
    "return",
    "return.cleanup",
    "falloff",
    "escape",
];

/// Canonical type-constructor spellings in `Ty` declaration order — the
/// closed type vocabulary of the format. [`type_spelling`] is the
/// authoritative per-constructor mapping (its exhaustive match forces a
/// spelling decision for every new constructor); this inventory mirrors it
/// for schema tests and generated capability tables.
pub const TYPE_SPELLINGS: &[&str] = &[
    "Int",
    "UInt",
    "Bool",
    "StringLiteral",
    "Float64",
    "None",
    "Never",
    "IntLiteral",
    "FloatLiteral",
    "Infer",
    "DType",
    "func",
    "generic_func",
    "overload",
    "param",
    "assoc",
    "dependent_index",
    "Self",
    "struct_type",
    "simd",
    "Error",
    "comptime_list",
    "tuple",
    "runtime_pack",
    "variadic_pack",
    "variant",
    "pointer",
    "ref",
];

pub const RESERVED_WORDS: &[&str] = &[
    "absent",
    "artifact",
    "blocks",
    "bool",
    "captures",
    "decls",
    "false",
    "features",
    "files",
    "fn",
    "functions",
    "locations",
    "none",
    "present",
    "registers",
    "struct",
    "structs",
    "true",
    "types",
    "vars",
];

pub fn version_header() -> String {
    format!("{MAGIC} {VERSION_MAJOR}.{VERSION_MINOR}")
}

pub fn instruction_mnemonic(instruction: &MirInstr) -> &'static str {
    match instruction {
        MirInstr::EstablishLoans { .. } => "loans.establish",
        MirInstr::InvalidateInteriors { .. } => "interiors.invalidate",
        MirInstr::MakeRef { .. } => "ref.make",
        MirInstr::ReadRef { .. } => "ref.read",
        MirInstr::CopyValue { .. } => "value.copy",
        MirInstr::WriteRef { .. } => "ref.write",
        MirInstr::MakeClosure { .. } => "closure.make",
        MirInstr::KeepAlive { .. } => "lifetime.keep_alive",
        MirInstr::Const { .. } => "const",
        MirInstr::SizeOf { .. } => "layout.size_of",
        MirInstr::ConstructTypeParam { .. } => "type.construct",
        MirInstr::MaterializeLiteral { .. } => "literal.materialize",
        MirInstr::UseVar { .. } => "var.use",
        MirInstr::MovePlace { .. } => "place.move",
        MirInstr::DefVar { .. } => "var.store",
        MirInstr::UnOp { .. } => "unary",
        MirInstr::BinOp { .. } => "binary",
        MirInstr::Call { .. } => "call",
        MirInstr::CallIndirect { .. } => "call.indirect",
        MirInstr::MethodCall { .. } => "call.method",
        MirInstr::PointerStorageTake { .. } => "pointer.take",
        MirInstr::PointerStorageDestroy { .. } => "pointer.destroy",
        MirInstr::UninitStorage { .. } => "uninit.make",
        MirInstr::UninitStorageTake { .. } => "uninit.take",
        MirInstr::UninitStorageDestroy { .. } => "uninit.destroy",
        MirInstr::GetField { .. } => "field.get",
        MirInstr::Index { .. } => "index.get",
        MirInstr::Slice { .. } => "slice.get",
        MirInstr::MultiIndex { .. } => "index.multi",
        MirInstr::MultiSet { .. } => "index.multi_set",
        MirInstr::Store { .. } => "place.store",
        MirInstr::StoreRef { .. } => "place.store_ref",
        MirInstr::LoadPlace { .. } => "place.load",
        MirInstr::MakeTuple { .. } => "tuple.make",
        MirInstr::MakeVariant { .. } => "variant.make",
        MirInstr::VariantIs { .. } => "variant.is",
        MirInstr::VariantGet { .. } => "variant.get",
        MirInstr::VariantSet { .. } => "variant.set",
        MirInstr::VariantTake { .. } => "variant.take",
        MirInstr::VariantSetInitWith { .. } => "variant.set_init_with",
        MirInstr::VariantDeinitWith { .. } => "variant.deinit_with",
        MirInstr::VariantReplace { .. } => "variant.replace",
        MirInstr::MakeSimd { .. } => "simd.make",
        MirInstr::SimdCast { .. } => "simd.cast",
        MirInstr::SimdShuffle { .. } => "simd.shuffle",
        MirInstr::Raise { .. } => "raise",
        MirInstr::Try { .. } => "try",
        MirInstr::Drop { .. } => "drop.reg",
        MirInstr::DropVar { .. } => "drop.var",
        MirInstr::ConsumeVar { .. } => "consume.var",
        MirInstr::ConsumePlace { .. } => "consume.place",
        MirInstr::Unsupported(_) => "unsupported",
        MirInstr::GetIter { .. } => "iter.init",
        MirInstr::HasNext { .. } => "iter.has_next",
        MirInstr::Next { .. } => "iter.next",
        MirInstr::TryNext { .. } => "iter.try_next",
    }
}

pub fn terminator_mnemonic(terminator: &MirTerm) -> &'static str {
    match terminator {
        MirTerm::Jump(_) => "jump",
        MirTerm::Branch { .. } => "branch",
        MirTerm::Return(_) => "return",
        MirTerm::ReturnWithCleanup { .. } => "return.cleanup",
        MirTerm::FallOff => "falloff",
        MirTerm::EscapeJump { .. } => "escape",
    }
}

pub fn use_mode_spelling(mode: UseMode) -> &'static str {
    match mode {
        UseMode::Copy => "copy",
        UseMode::Move => "move",
        UseMode::BorrowShared => "borrow_shared",
        UseMode::BorrowMut => "borrow_mut",
    }
}

pub fn projection_spelling(projection: &Proj) -> &'static str {
    match projection {
        Proj::Field(_) => "field",
        Proj::Index(_) => "index",
        Proj::ConstIndex(_) => "const_index",
        Proj::Variant(_) => "variant",
        Proj::UninitPayload => "uninit_payload",
    }
}

pub fn intrinsic_subscript_spelling(intrinsic: MirIntrinsicSubscript) -> &'static str {
    match intrinsic {
        MirIntrinsicSubscript::TupleStorage => "tuple_storage",
        MirIntrinsicSubscript::VariadicStorage => "variadic_storage",
        MirIntrinsicSubscript::Simd => "simd",
        MirIntrinsicSubscript::Pointer => "pointer",
        MirIntrinsicSubscript::ComptimeList => "comptime_list",
    }
}

pub fn constant_spelling(constant: &Const) -> &'static str {
    match constant {
        Const::Int(_) => "int",
        Const::Float(_) => "float",
        Const::IntLiteral(_) => "int_literal",
        Const::FloatLiteral(_) => "float_literal",
        Const::Bool(_) => "bool",
        Const::Str(_) => "string",
        Const::Function(_) => "function",
        Const::None => "none",
    }
}

pub fn type_spelling(ty: &Ty) -> &'static str {
    match ty {
        Ty::Int => "Int",
        Ty::UInt => "UInt",
        Ty::Bool => "Bool",
        Ty::StringLiteral => "StringLiteral",
        Ty::Float64 => "Float64",
        Ty::None => "None",
        Ty::Never => "Never",
        Ty::IntLiteral => "IntLiteral",
        Ty::FloatLiteral => "FloatLiteral",
        Ty::Infer => "Infer",
        Ty::Dtype => "DType",
        Ty::Func { .. } => "func",
        Ty::GenericFunc { .. } => "generic_func",
        Ty::Overload(_) => "overload",
        Ty::Param { .. } => "param",
        Ty::Assoc { .. } => "assoc",
        Ty::Dependent(_) => "dependent_index",
        Ty::SelfType => "Self",
        Ty::Struct(_, _) => "struct_type",
        Ty::Simd { .. } => "simd",
        Ty::Error => "Error",
        Ty::ComptimeList(_) => "comptime_list",
        Ty::Tuple(_) => "tuple",
        Ty::RuntimePack(_) => "runtime_pack",
        Ty::VariadicPack(_) => "variadic_pack",
        Ty::Variant(_) => "variant",
        Ty::Pointer { .. } => "pointer",
        Ty::Ref(_) => "ref",
    }
}

pub fn is_bare_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
        && !RESERVED_WORDS.contains(&value)
}

pub fn quote(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                use std::fmt::Write;
                write!(output, "\\u{{{:x}}}", character as u32)
                    .expect("writing to a String cannot fail");
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

/// Candidate source-map paths for one verifier finding, most precise first
/// and always ending at the artifact root.
///
/// The canonical verifier locates findings textually with the prefixes
/// `MIR function '<name>' block <n>` and `MIR function '<name>'` (see the
/// module documentation of `crate::mir::verify`; this is the only consumer
/// of that spelling). The name is matched against the program's own function
/// identities — longest match wins — instead of re-parsing quote syntax, so
/// a name containing a quote cannot mislocate a finding.
fn finding_candidates<'a>(names: impl IntoIterator<Item = &'a str>, message: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    if let Some(rest) = message.strip_prefix("MIR function '") {
        let matched = names
            .into_iter()
            .filter(|name| {
                rest.strip_prefix(*name)
                    .is_some_and(|after| after.starts_with('\''))
            })
            .max_by_key(|name| name.len());
        if let Some(name) = matched {
            let after = &rest[name.len() + 1..];
            if let Some(block) = after.strip_prefix(" block ") {
                let digits = &block[..block
                    .find(|character: char| !character.is_ascii_digit())
                    .unwrap_or(block.len())];
                if !digits.is_empty() {
                    candidates.push(format!("function/{name}/bb{digits}"));
                }
            }
            candidates.push(format!("function/{name}"));
        }
    }
    candidates.push("artifact".to_string());
    candidates
}

/// The first candidate path the source map locates. A decoded artifact always
/// marks its root, so the terminal `artifact` candidate resolves; the empty
/// span only backstops a hand-built map.
fn resolve_candidate_span(source_map: &ArtifactSourceMap, candidates: &[String]) -> (String, Span) {
    for path in candidates {
        if let Some(span) = source_map.span(path) {
            return (path.clone(), span);
        }
    }
    ("artifact".to_string(), (0, 0))
}

mod parse;
mod write;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_and_quotes_are_canonical() {
        assert!(is_bare_identifier("alpha_2"));
        assert!(!is_bare_identifier("fn"));
        assert!(!is_bare_identifier("has.dot"));
        assert_eq!(quote("a\n\"\\\u{7}"), "\"a\\n\\\"\\\\\\u{7}\"");
    }

    #[test]
    fn finding_candidates_rank_block_function_then_artifact() {
        assert_eq!(
            finding_candidates(
                ["main"],
                "MIR function 'main' block 12: invalid register r9"
            ),
            ["function/main/bb12", "function/main", "artifact"]
        );
        assert_eq!(
            finding_candidates(
                ["main"],
                "MIR function 'main' has 1 variable names for 2 slots"
            ),
            ["function/main", "artifact"]
        );
        assert_eq!(
            finding_candidates(["main"], "program-level finding"),
            ["artifact"]
        );
        // Unknown names and malformed block numbers never invent paths.
        assert_eq!(
            finding_candidates(["main"], "MIR function 'other' block 0: x"),
            ["artifact"]
        );
        assert_eq!(
            finding_candidates(["main"], "MIR function 'main' block x: y"),
            ["function/main", "artifact"]
        );
    }

    #[test]
    fn finding_candidates_match_quoted_names_longest_first() {
        assert_eq!(
            finding_candidates(
                ["tricky", "tricky' block 2"],
                "MIR function 'tricky' block 2' block 0: x"
            ),
            [
                "function/tricky' block 2/bb0",
                "function/tricky' block 2",
                "artifact"
            ]
        );
    }

    #[test]
    fn candidate_resolution_falls_back_to_the_artifact_root() {
        let parsed = parse_artifact(
            b"mojito-mir 1.0\nartifact { features: [], files: [], structs: [], decls: [], functions: [] }",
            "empty.mir",
        )
        .expect("parse empty artifact");
        let candidates = [
            "function/missing/bb3".to_string(),
            "function/missing".to_string(),
            "artifact".to_string(),
        ];
        let (path, span) = resolve_candidate_span(&parsed.source_map, &candidates);
        assert_eq!(path, "artifact");
        assert_eq!(span, parsed.source_map.span("artifact").unwrap());
    }
}

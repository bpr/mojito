# Textual MIR Assembler Parser and Diagnostics Plan

> **Status:** the seed-subset deferral recorded below closed with the
> full-schema parser and corpus round trips; see
> [`roundtrip-plan.md`](roundtrip-plan.md).

## Objective

Implement a source-located parser for Mojito MIR 1.x artifacts that reconstructs
the complete in-memory `MirProgram` represented by
[`mir-text-format.md`](mir-text-format.md). The parser must be independent of the
Mojo lexer, parser, AST, checker, and source-module loader, and every rejection
must identify the relevant byte range in the artifact.

This milestone owns UTF-8 decoding, artifact lexing, syntactic recovery,
schema-shaped decoding, identity reconstruction, and structural diagnostics. It
does **not** run `mir::verify`, ownership analysis, drop elaboration, the VM, or
claim lossless semantic round trips. Those remain the next roadmap tasks.

## Resolve the schema/printer contract first

Before building the parser, audit `docs/mir-text-format.md`, `mir::text`'s closed
vocabulary, the canonical writer, and all disassembler snapshots as one format.
Correct ambiguities in the schema and printer together, then pin them in
`mir_text_schema_test.rs`.

The audit must explicitly resolve:

- dotted record tags such as `loans.establish` and `call.indirect`, which are
  emitted canonically but are not covered by the current `bare` production;
- the exact field names and order emitted for every Rust structure, including
  fields whose canonical names intentionally differ from Rust member names;
- all nullary, positional, and named tags used by types, constraints, origins,
  capture sets, transfers, call contracts, and nested `try` regions;
- whether a file identity is `(path, module)` in the current MIR model and how an
  absent path/module is reconstructed without inventing source provenance;
- the version-1 policy for unknown features and unknown fields inside an
  explicitly extensible `optional { ... }` record.

The implemented grammar, canonical disassembler output, and documentation must
agree before parser fixtures are frozen. Do not make the parser accept an
undocumented second dialect merely to accommodate a writer/schema mismatch.

## Public API and ownership

Keep the public boundary in `src/mir/text.rs`, ordered public-first:

```rust
pub fn parse_artifact(
    input: &[u8],
    source_name: impl Into<String>,
) -> Result<ParsedArtifact, ArtifactReport>;

pub struct ParsedArtifact {
    pub program: MirProgram,
    pub source_map: ArtifactSourceMap,
}

pub struct ArtifactReport {
    pub diagnostics: Vec<ArtifactDiagnostic>,
}

pub struct ArtifactDiagnostic {
    pub span: Span,
    pub message: String,
    pub context: Vec<String>,
}
```

Exact accessor/constructor details may be adjusted to preserve repository API
style, but retain these properties:

- accept bytes so invalid UTF-8 and a BOM receive artifact diagnostics rather
  than being rejected before the parser;
- accept a diagnostic source name without treating it as a Mojo module path;
- return a `ParsedArtifact`, not a bare `MirProgram`, so the following artifact
  verifier task can map semantic findings back to assembly source;
- keep artifact-source spans out of `MirProgram::SpanTable`, whose spans describe
  original Mojo source locations serialized *inside* the artifact;
- implement `Display`/`Error` for the report without flattening the structured
  diagnostics or losing individual spans.

Put private implementation under `src/mir/text/`:

- `lex.rs` — UTF-8 validation and artifact tokens;
- `parse.rs` — delimiter-aware syntax parsing and recovery;
- `decode.rs` — exhaustive typed construction of MIR and semantic records;
- `source.rs` — artifact paths/node identities and source-map storage.

Do not reuse `Lexer`, `Parser`, Mojo `Token`, Mojo `ParseError`, Serde, Rust
`Debug`, or source AST reconstruction.

## Artifact source map

Define stable artifact node paths that survive typed decoding and do not depend
on allocation addresses. At minimum retain spans for:

- the version header and artifact envelope;
- each file, struct declaration, function declaration, and function;
- every function/region block, instruction, and terminator;
- identity-bearing table entries (`var_type`, `reg_type`, `reg_loc`);
- each typed record field and nested semantic value when a later verifier
  finding may refer to it.

Use canonical identities in paths where they already exist (function symbol,
`bbN`, `%rN`, `$vN`, declaration symbol), and vector indexes only for ordered
anonymous children. Nested `try` paths must include the region arm and nesting
chain so local `bbN` namespaces cannot collide.

The source map is diagnostic metadata, not part of program equality or
disassembly. It must be possible for the later verifier integration to choose
the narrow operand/field span first and fall back through instruction, block,
function, then whole-artifact spans.

## Phase 1: byte lexer

Implement a small, non-layout-sensitive lexer over UTF-8 byte offsets.

Tokenize:

- words/tags, punctuation, colon, comma, brackets, braces, and parentheses;
- `%rN`, `$vN`, `bbN`, and `fileN` as distinct identity tokens retaining the
  numeric spelling and span;
- canonical unsigned/signed decimal numbers and fixed-width lowercase hex bit
  strings without prematurely converting them;
- quoted strings with the schema escapes;
- comments beginning with `#` through LF;
- the exact `mojito-mir` header spelling and `major.minor` version.

Diagnose invalid UTF-8, BOMs, invalid/control characters, unterminated strings,
unknown escapes, malformed Unicode scalar escapes, overflow-length identities,
and unexpected punctuation. Continue after recoverable token errors, with a
fixed diagnostic cap and guaranteed cursor progress.

Preserve raw spans for numeric and string tokens. Exact integer/float literal
parsing belongs to typed decoding so diagnostics can name the expected domain.

## Phase 2: schema syntax parser

Parse tokens into a private, spanned schema value tree rather than constructing
MIR directly from token lookahead:

```text
Value = Word | String | Number | Identity
      | Positional(tag, value)
      | Record(tag, fields)
      | List(values)
```

Each value, record field name, and delimiter owns a byte span. This intermediate
tree provides one reusable recovery layer and lets typed decoding report
duplicate, missing, misplaced, and unknown fields precisely without duplicating
delimiter logic across every MIR variant.

Recovery rules:

- synchronize within records/lists at a comma or the matching closing
  delimiter;
- synchronize the top level at the header or `artifact` record;
- report mismatched delimiters at the closer while retaining the opener span as
  context;
- reject trailing non-comment tokens after the artifact;
- never insert a value that could silently change program meaning.

The syntax tree is private and deliberately not a generic public data format.

## Phase 3: exhaustive typed decoder

Decode the spanned value tree into `MirProgram` with small typed helpers such as
`required_field`, `option`, `list`, `uint`, `symbol`, `reg`, `var`, `block`, and
`tagged_record`.

Implement wildcard-free matches for the same complete inventory owned by the
disassembler:

1. version/envelope, files, declarations, functions, tables, blocks, and nested
   `try` regions;
2. every `MirInstr`, `MirTerm`, place/projection, loan, interior, capture, and
   invalidation form;
3. direct, indirect, method, subscript, and iterator contracts with every
   checked field;
4. every `Const`, `CheckedConst`, `CtValue`, `CtExpr`, `Ty`, `TyArg`,
   `ParamDecl`, constraint, callable default, and transfer;
5. origins, pointer origins, mutabilities, reference signatures, callable
   environments, capture sets, DTypes, operators, conventions, slice kinds,
   use modes, intrinsics, and result adapters.

Adding a schema/Rust variant must fail compilation until both writer and decoder
matches are updated.

### Exact reconstruction

- Parse concrete floats from exactly 16 lowercase hexadecimal bits with
  `f64::from_bits`; never parse them through decimal `f64` text.
- Use `IntLiteral` and `FloatLiteral` exact parsers for arbitrary-precision and
  negative-zero-preserving literals.
- Range-check every conversion to `u32`, `usize`, `i64`, and enum width at its
  token span.
- Preserve declared vector order and numeric identities; do not renumber.
- Rebuild maps/sets only after duplicate entries have been diagnosed.
- Resolve `fileN` through the declared file table while rebuilding serialized
  Mojo `SourceSpan`s; `SyntaxId` remains absent by schema design.
- Construct `MirProgram::invariant_errors` empty. Parser/shape failures belong
  in `ArtifactReport`, and semantic findings belong to the later verifier task.

## Structural diagnostics owned by this task

The decoder must reject and locate:

- unsupported major versions and unsupported required minor-version content;
- duplicate/missing/unknown required fields and wrong record tags;
- wrong value kinds, invalid option syntax, and invalid enum spellings;
- malformed or overflowing numbers and exact literals;
- duplicate or non-dense `fileN`/`bbN` identities;
- duplicate function/declaration names and duplicate numeric map entries;
- out-of-range file references and reversed serialized source spans;
- explicit counts inconsistent with table/list lengths;
- parameter masks/declaration vectors with inconsistent lengths;
- place projection/type vector length mismatches;
- instruction/terminator field-shape errors and invalid local block references
  that can be established without semantic typing.

Do not duplicate type/effect/reference/call-contract policy from `mir::verify`.
For example, a syntactically valid instruction whose operand register has the
wrong type may parse successfully; artifact verifier integration will reject it
with the preserved source map in the next task.

## Version and extension behavior

- Require the exact magic header and reject unknown major versions.
- Accept version 1.0 exactly.
- For a newer version-1 minor, accept only additions explicitly permitted by the
  compatibility section: named features and unknown fields inside an
  `optional { ... }` record.
- Never ignore an unknown required field, instruction, terminator, type, or
  semantic tag.
- Preserve no unknown executable data in `MirProgram`; if it cannot be safely
  skipped under the schema, reject it.

Pin this behavior with tests before any later version exists so compatibility
does not become an ad hoc parser decision.

## Focused tests and fixtures

Add `tests/mir_assembler_test.rs` and artifact fixtures under
`tests/fixtures/mir/` (or the repository's established equivalent).

Positive tests:

- parse every checked-in disassembler snapshot;
- inspect reconstructed functions, declarations, tables, instructions, exact
  literals, origins, calls, and nested regions rather than relying only on
  parse success;
- accept whitespace, comments, trailing list/record commas, Unicode strings,
  and every canonical escape;
- prove identity numbers and ordered lists are preserved;
- prove artifact source-map paths point at exact instruction, operand, and
  nested-region substrings;
- parse a canonical artifact, disassemble the resulting MIR, and compare with
  the canonical input only as a parser smoke test. Full semantic equivalence and
  corpus-wide round trips remain the later **Lossless round trips** task.

Negative diagnostic fixtures should pin byte ranges and messages for:

- invalid UTF-8/BOM, header/version errors, unterminated strings, invalid escapes,
  bad Unicode scalars, comments at EOF, and mismatched delimiters;
- missing, duplicate, unknown, and mistyped fields;
- unknown mnemonics/types/enum tags and malformed identities;
- numeric overflow, malformed float bits, invalid exact literals, duplicate map
  keys, non-dense blocks/files, dangling file IDs, and reversed spans;
- multiple independent recoverable errors in one artifact, capped recovery, and
  no infinite-loop regression.

Keep diagnostics as stable structured expectations `(span, message, context)`;
avoid snapshots containing terminal colors or platform-specific absolute paths.

## Explicitly skipped unrelated tests

Validation is limited to the text schema, disassembler/parser boundary, and any
direct MIR tests needed to construct fixtures. Do **not** run corpus, stdlib,
evaluator/VM, checker, Mojo parser, module-linking, conformance, ownership, or
drop suites unless a focused failure identifies one as the owning target.

Run:

```text
cargo fmt --check
cargo nextest run --test mir_text_schema_test
cargo nextest run --test mir_disassembler_test
cargo nextest run --test mir_assembler_test
cargo clippy --all-targets --all-features -- -D warnings
git diff --check
```

If `scripts/check` cannot exclude unrelated suites, do not run it for this task.
Record the focused-test policy in the handoff rather than falling back to the
full suite.

## Documentation and lifecycle

- Update `docs/mir-text-format.md` for every ambiguity resolved by the initial
  schema/printer audit.
- Update `docs/architecture.md` with the byte lexer → spanned value tree → typed
  decoder pipeline and the parser/verifier boundary.
- Update `docs/symbol-map.md` for the public API, diagnostics, source map, and
  private module ownership.
- Update `docs/features.md` to mark parsing/structural diagnostics supported
  while artifact semantic verification and execution remain unsupported.
- Remove the completed **Assembler parser and diagnostics** roadmap entry rather
  than checking it off.
- Update `commit_msg.txt` on completion.

## Acceptance criteria

- Every canonical Mojito MIR 1.0 artifact emitted by the current disassembler
  parses into a complete `MirProgram` without Mojo frontend dependencies.
- The parser accepts only the documented artifact language and implements the
  declared major/minor compatibility policy.
- Every syntax and structural failure has an exact artifact byte span, useful
  context, bounded recovery, and no partial successful result.
- Exact numeric values, symbolic identities, ordered collections, file/source
  locations, and all checked semantic metadata reconstruct without loss.
- `ParsedArtifact` retains enough source mapping for the following verifier task
  to locate semantic findings without modifying `MirProgram::SpanTable`.
- The parser does not call `mir::verify`, ownership, drops, a backend, or the
  Mojo lexer/parser.
- Focused schema/disassembler/assembler tests, formatting, Clippy with warnings
  denied, and `git diff --check` pass; unrelated suites are explicitly skipped.

# Text Format Schema Implementation Plan

## Objective

Specify the first version of Mojito's textual MIR/VM assembly format as a
normative, deterministic, standalone artifact contract. The schema must cover
all verified `MirProgram` data needed by the VM, ownership/drop analysis, and
future native backends without reconstructing Mojo source AST facts.

This roadmap item defines the language and its compatibility rules. It does not
implement the disassembler, assembler parser, artifact verifier entry point,
round-trip harness, or CLI execution; those remain the subsequent roadmap
tasks.

## Deliverables

1. Add `docs/mir-text-format.md` as the normative format specification.
2. Update `docs/architecture.md`, `docs/vm-instruction-set.md`, and
   `docs/symbol-map.md` to point to the specification and state ownership of the
   schema.
3. Add a small schema vocabulary module, tentatively `src/mir/text.rs`, only for
   stable public constants and closed spelling tables shared by future printer
   and parser work. It must not serialize or parse programs yet.
4. Add `tests/mir_text_schema_test.rs` to pin version constants, reserved words,
   mnemonic uniqueness, and exhaustive coverage of closed MIR enums.
5. Remove the completed **Text format schema** entry from `roadmap.md` and
   update `commit_msg.txt` when the implementation is complete.

## Format Decisions to Freeze

### 1. Artifact envelope and versioning

- Use a required first line such as `mojito-mir 1.0` with separate major and
  minor integers.
- A major change may alter or remove syntax or semantics. A minor change may
  add explicitly skippable metadata but may not add executable instructions to
  an existing major version without updating consumers.
- Reject an unknown major version. Define how a consumer handles a newer minor
  version and unknown optional sections.
- Require UTF-8 input, `\n` logical newlines, and deterministic trailing-newline
  behavior.
- Reserve a feature/capability declaration in the envelope so later revisions
  can gate optional semantics without guessing from instruction presence.

### 2. Lexical grammar

- Define ASCII keywords and mnemonics, identifiers, decimal unsigned indexes,
  signed integers, exact integer literals, exact finite floating literals,
  quoted UTF-8 strings, comments, punctuation, and whitespace.
- Specify one canonical escaping algorithm for strings, symbols, source paths,
  field names, keyword names, and diagnostic messages. Do not reuse Mojo source
  literal parsing implicitly.
- Distinguish bare identifiers from quoted symbols so arbitrary linked/mangled
  names remain lossless.
- Reserve keyword and mnemonic namespaces up front; unknown executable forms
  are errors, not ignorable extensions.

### 3. Deterministic identifiers and ordering

- Spell registers as `%rN`, variable slots as `$vN`, blocks as `bbN`, functions
  as stable quoted symbols, and source files as `fileN` table entries.
- Preserve numeric register, slot, and block identities exactly; parsing must
  not renumber them.
- Print functions in `MirProgram::functions` order after requiring unique
  names, declarations in a documented canonical symbol order, struct fields in
  declaration order, map-like metadata by numeric key, and set-like metadata in
  lexical order.
- Specify stable ordering for nested `try` regions and their local block IDs.
- Exclude `MirProgram::invariant_errors`: an artifact represents a candidate
  program to verify, not cached findings from an earlier verifier run.

### 4. Top-level declarations

Define complete syntax for:

- struct declarations: name, ordered fields, mutable-self method set,
  fieldwise-init flag, compile-time parameter declarations, explicit-destroy
  message, and named destructor effects;
- function declarations: lowered symbol, parameter names/types,
  defaults/required mask, positional and keyword variadic types, their retained
  conventions and ABI indexes, positional/keyword marker indexes, compile-time
  declarations, receiver presence/convention, fixed-parameter conventions,
  return/reference ABI, raising/error contract, and write-back mask;
- call-local erased abstract requirements and callable `Ty::Func` /
  `Ty::GenericFunc` contracts, explicitly identified as declarations of record
  where no concrete function declaration exists.

Document every alignment invariant already enforced by `mir::verify` rather
than leaving consumers to infer parallel-vector relationships.

### 5. Type and compile-time value grammar

- Enumerate every `Ty` variant, including scalar/literal types, tuples,
  variants, nominal structs with `TyArg`s, references, origin-bearing pointers,
  dependent types, associated projections, overload sets, callable and generic
  callable contracts, packs, and error/never forms.
- Specify `ParamDecl`, `CallableDefault`, `GenericConstraint`, `CtValue`,
  `CheckedConst`, `TyArg`, callable environment/capture sets, and all origin and
  signature-origin forms used transitively by declarations or instructions.
- Make binder identities numeric and declaration-scoped where the in-memory
  model uses stable IDs; never recover identity from source parameter spelling.
- Define canonical ordering for unions, capture sets, constraints, and other
  semantically set-like forms.
- State numeric fidelity requirements: arbitrary-precision integers and exact
  finite floats must not round-trip through host `i64`/`f64` text conversions.

### 6. Functions, blocks, and source locations

- Define a function header containing register/slot counts, ordered slot names,
  parameter count and types, ownership/deinit/write-back masks, slot type table,
  return/reference ABI, and raising contract.
- Require one terminator per block and define the block namespace for nested
  structured `try` regions versus enclosing-function `EscapeJump` targets.
- Encode register types explicitly, independently of instruction result syntax,
  so verification never re-infers them.
- Normalize `SourceSpan` data into an artifact-level file table plus byte
  offsets and optional origin slot. Preserve absent module/path provenance
  explicitly, omit `SyntaxId`, and state that byte offsets address the named
  UTF-8 source file when source text is available.
- Define whether generated/no-source instructions use an absent location or a
  distinguished synthetic location; do not overload `(0, 0)`.

### 7. Places, loans, and interiors

- Specify the full typed `MirPlace`: root slot/type, every projection and its
  resulting type, terminal type, and optional `through` capability slot.
- Cover field, dynamic index, constant index, variant payload, and uninitialized
  payload projections.
- Specify `MirLoan`, mutable permission, `MirInteriorOrigin`, destination
  interior domains, invalidation exceptions, and capture-access paths.
- State the verifier obligations connecting executable places, through
  capabilities, declared reference origins, mutable permissions, and canonical
  interior roots. These fields are semantic artifact data even when erased by
  VM execution.

### 8. Instructions and terminators

- Assign one unique canonical mnemonic to every `MirInstr` and `MirTerm`
  variant currently listed in `src/mir/ir.rs`.
- Define operand order and named metadata for every form, including calls,
  subscript contracts, iterator adapters, closures/captures, SIMD, variants,
  uninitialized storage, structured exceptions, ownership/drop operations, and
  unsupported boundaries.
- Keep result register typing in the explicit register-type table rather than
  relying on mnemonic suffixes.
- Encode optional values unambiguously (`none` versus an empty list or empty
  string) and use length-delimited lists/maps to avoid parse ambiguity.
- State that instructions unknown to the selected schema major version are
  fatal even if a parser could skip their syntax.

## Implementation Sequence

1. Inventory all transitive schema types from `MirProgram`, producing a
   checklist in `docs/mir-text-format.md`. Cross-check `MirInstr`, `MirTerm`,
   `Ty`, origins, checked call records, compile-time declarations/defaults, and
   source metadata.
2. Write the lexical grammar and artifact envelope first, including version and
   compatibility behavior, escaping, comments, and identifier namespaces.
3. Specify top-level declaration and function/block structure with a complete
   representative artifact showing declarations, typed slots/registers,
   locations, blocks, and a simple call.
4. Specify the recursive type/origin/constraint grammar and include examples of
   a generic callable, a reference result, an origin-bearing pointer, and a
   heterogeneous runtime pack boundary.
5. Specify places, projections, loans, interior identities, captures, and
   source-location tables with examples that exercise `through` and interior
   metadata.
6. Define every instruction and terminator spelling in a machine-readable-ish
   table whose entries map one-to-one to Rust variants; reconcile this table
   with `docs/vm-instruction-set.md` rather than maintaining two conflicting
   mnemonic lists.
7. Add `mir::text` version constants and closed spelling tables. Keep this
   module dependency-light and ordered public-first; it is the future shared
   vocabulary for the disassembler and parser.
8. Add exhaustive tests that fail when a closed enum gains a variant without a
   schema spelling. Test keyword/mnemonic uniqueness, version formatting, and
   representative escaping/source-location examples without implementing a
   general parser.
9. Update architecture/navigation docs, remove the roadmap entry, update
   `commit_msg.txt`, and run the required repository gates.

## Test Strategy

- Unit tests beside `mir::text` for version constants, identifier classification,
  escaping examples, and uniqueness of reserved spellings.
- `tests/mir_text_schema_test.rs` for exhaustive mappings from every MIR
  instruction/terminator and closed supporting enum to its normative spelling.
- Documentation examples covering at least:
  - a minimal function and declaration;
  - a typed projected place with `through`;
  - an abstract trait-dispatch call with `ref`-to-`read` narrowing;
  - `Next`/`TryNext` with `CopyIteratorReference`;
  - a generic indirect callable contract;
  - a structured `try` region with `EscapeJump`;
  - exact numeric literals and multi-file source locations.
- A schema inventory test or explicit checklist ensuring every transitive
  artifact type is classified as serialized, derived, or deliberately omitted.
- Defer golden disassembly and parse/round-trip tests to their named roadmap
  tasks.

## Acceptance Criteria

- `docs/mir-text-format.md` is sufficient for an independent implementation to
  emit or parse every current verified `MirProgram` field without consulting
  Mojo AST syntax.
- Every `MirInstr`, `MirTerm`, declaration field, type/origin form, constant,
  place projection, call contract, and source-location form has exactly one
  specified representation.
- Deterministic ordering and string/numeric escaping are normative, not printer
  implementation details.
- Version compatibility, unknown syntax, optional metadata, and verifier
  expectations are explicit.
- Cached/compiler-only data is identified and excluded deliberately.
- Schema vocabulary tests make future MIR growth fail visibly until the schema
  is updated.
- `cargo fmt --check`, targeted schema tests, `cargo nextest run --profile quick`,
  `env RUSTC_WRAPPER= scripts/check`, Clippy with warnings denied, and
  `git diff --check` pass before completion.

## Risks and Guardrails

- Avoid deriving the public grammar mechanically from Rust `Debug`; Rust field
  names and enum layout are not a compatibility promise.
- Do not add Serde or choose JSON/TOML as an accidental schema. The roadmap
  calls for a reviewable assembly syntax aligned with the VM mnemonics.
- Do not make source text mandatory for execution; locations are diagnostic
  provenance and may refer to unavailable files.
- Do not omit ownership-only metadata merely because the VM erases it. Assembled
  artifacts must pass the same verifier and ownership analysis as lowered MIR.
- Do not freeze vector/map iteration order from `HashMap`; every emitted order
  must be specified independently.
- Keep the schema task specification-focused so it does not absorb the distinct
  disassembler and assembler-parser roadmap work.

# Architecture After Parsing

Companion references: [feature support](features.md),
[symbol ownership and navigation](symbol-map.md), and the
[VM instruction set](vm-instruction-set.md).

This document describes mojito after parsing. Its simplest input is the parsed
program:

```rust
Vec<ast::Stmt>
```

When the compiler is running from a file path, the first post-parse stage may
also parse imported modules and link them into that same shape. Lexing and
parsing are intentionally out of scope here; see [the frontend guide](frontend.md).
This file starts where the parser stops and follows a program through module
linking, compile-time elaboration, semantic checking, HIR lowering, MIR lowering,
compiler analyses, drop elaboration, and execution on the register VM.

## Workspace Layout

Since the 2026-09 crate split, each pipeline phase is its own workspace crate
under `crates/`, and the root `mojito` crate is the facade: the driver
(`compiler::Compiler`, `artifact`), the CLI (`main.rs`), and a `lib.rs` that
re-exports every crate at its historical module path (`mojito::checker`,
`mojito::mir`, ...), so consumers and tests are unaffected by the split.

Dependency direction is enforced by Cargo. Bottom-up: `mojito-common`
(token/literal/errors) → `mojito-ast` (+ structural call binding) →
`mojito-types` (Ty/origins/ct) → `mojito-symbol` → `mojito-checked` →
{`mojito-lexer` → `mojito-parser` → `mojito-module`, `mojito-hir`,
`mojito-native-core`} → `mojito-checker` → `mojito-mir` → `mojito-analysis` →
`mojito-vm` → {`mojito-native` → `mojito-pliron` (feature-gated),
`mojito-comptime`} → the root facade. Note that the crate DAG is not the
pipeline order: `mojito-native` sits **above** the VM because the elaborator
owns its compile-time executor (`VmBackend::call_concrete`), as upstream's
elaborator owns its interpreter, and `mojito-comptime` sits above the
checker because it elaborates the program the check consumes. `crates/mojito-runtime` remains the
independently versioned native C-ABI runtime, depending on nothing above.
`mojito-checked` also owns the checked declaration metadata MIR carries
(`declaration`: `StructDeclaration` and `FunctionDeclaration`, which `mir`
re-exports as `MirStructDeclaration` and `MirFunctionDeclaration`) and a
compiled package's declaration records (`package`), so the checker and MIR
read one encoding of a library declaration with no edge between them.

## Big Picture

The post-parse pipeline is:

```text
Vec<Stmt>
  -> module link
  -> source validation
  -> comptime elaboration
  -> check
  -> HIR CFG
  -> MIR
  -> ownership / borrow / liveness analysis
  -> drop elaboration
  -> pre-elaboration pipeline
  -> elaboration to concrete MIR (native::mono)
  -> post-elaboration pipeline
  -> register VM
```

Normal whole-program clients use `compiler::Compiler`, which embodies this
ordering rather than requiring callers to compose stages manually:

```rust
let compiler = Compiler::default();
let program = compiler.compile_path(path)?;
let execution = compiler.execute(&program)?;
```

`CompiledProgram` exposes the check's nonfatal diagnostics (`warnings()`,
the `Warning` list `CheckedProgram` gathers from its facts in source order;
the CLI prints them after a successful compile) and privately retains the
`CheckedProgram` and three MIR phases,
each named by its accessor and each computed once:

| Phase | Accessor | What it is | Who consumes it |
|---|---|---|---|
| Parametric MIR | `mir()` | The ownership-verified, pre-drop `MirProgram` the driver lowered; generic bodies name their parameters. With an imported package, the program's own functions and declarations, with the package's MIR declarations attached and each resolved the first time a lookup names it | Ownership analysis |
| Drop-elaborated MIR | `elaborated_mir()` | Parametric MIR with drops inserted and the pre-elaboration pipeline run, re-verified; may still be generic | The elaborator, beside an imported package's bodies |
| Whole drop-elaborated MIR | `drop_elaborated_mir()` | `elaborated_mir()` with every package body joined, decoded, drop-elaborated, and through the pre-elaboration pipeline; the same program when nothing is imported | `emit_mir` and the serialized artifact |
| Concrete MIR | `concrete_mir()` | The elaborator's entry-rooted output, a `SpecializedProgram` whose `mir::ConcreteMir` passed `verify_concrete` | The VM and the native backend |

### The Bundled Library Is An Imported Package

Mojo imports a precompiled package without reading its source again. The
bundled library is imported the same way, as one compiled package per
compiler build (`src/package.rs`):

```text
build (once per compiler build, then cached):
  every std module -> link (recording each module) -> prepare
    -> elaborate (exporting
    the module constants it folded) -> check (exporting each library
    declaration's rows) -> MIR -> ownership
    -> std package: interface + one record per library declaration
       (its MIR declarations and effect summaries, then the checker
       rows they do not say) + module table (each module's link
       record) + one checked record per lowering unit (its checked
       nodes and site facts; the build refuses a unit that, lowered
       alone, gives any function other than the library's MIR)

import (every compilation that links the bundled library):
  source -> link (each library module from its record)
    -> prepare -> elaborate (the program's own statements, the reached
       modules' declaration records and module constants as context)
    -> check (the reached modules' declaration records attached, each
       declaration's residue decoded on first lookup and joined with its
       MIR declarations; the program's own statements checked)
    -> MIR of the program's own declarations, with the package's
       declaration records attached (each decoded on first lookup)
    -> verify + ownership (own functions)
    -> drop elaboration + pre-elaboration pipeline (own functions)
    -> native::mono, which, when it first reaches a package body,
       lowers the body's unit (checked arena -> HIR CFG -> MIR),
       inserts its drops, and runs the pipeline then -> VM
```

- **The package file** (`mir::text::package`) is a header followed by
  bytecode records, as upstream's package is bytecode throughout. The
  header holds the package's identity (package, compiler build identity,
  library root) and the byte range of the
  declaration directory (`checked::package::DeclarationDirectory`). That
  directory holds every name once, in one string section, and places each
  library module once, by dotted name, with its link record, one record
  per library declaration, and one row per function, which names it once
  and holds its signature masks and the range of the unit it lowers
  from, as an MLIR function op names its symbol once and holds its
  region. The importer maps the
  file (`PackageBytes`), decodes its header, and reads the
  directory in place, looking a name up by binary search over the mapped
  rows; declarations and units decode from their own ranges on demand:
  Hello World decodes three of 251 declaration records and lowers three
  of 874 bodies.
- **The type section** (`mojito_types::type_section`) holds every type
  the records hold once, as MLIR bytecode's attribute and type section
  does: a record refers to a type by its index, and an entry holds the
  type's variant with each child an index of an earlier entry. The
  package write runs every record encode, the checker residues included,
  inside one `encoding_types` scope and appends the section last, its
  range in the header; the importer's `Payload` runs every record decode
  inside `decoding_types`, which decodes an entry the first time a record
  refers to it and shares the node with every later reference. An entry
  is keyed by its own bytes, so a decorated type never shares an entry
  with its undecorated twin. The section is 0.6 MB smaller than the
  inline types it replaced (2.54 MB to 1.98 MB), and decoding the units
  Hello World reaches takes about 25 ms rather than 30.
- **A unit's record** (`checked_unit::CheckedUnit`) is a package body as
  the check left it, as upstream's package holds each body's IR as its
  parser wrote it and its checks validated it, structured control flow
  included. A lowering unit (`CheckedProgram::lowering_units`) is a module
  `def`, one struct method (with its struct's name and the symbol it
  lowers to), an inherited default's wrapper, a struct's synthesized
  destructor, copy constructor, or move constructor
  (`CheckedProgram::{synthesizes_destructor, synthesizes_copy_constructor,
  synthesizes_move_constructor}`: a struct declaring no `__deinit__`,
  `__copyinit__`, or `__moveinit__` whose `IsTriviallyDeinitable`,
  `IsTriviallyCopyable`, or `IsTriviallyMovable` conformance is possible
  but not unconditional), a trait default, or a
  module constant's lifted applications; its record holds its checked
  tables (`CheckedProgram::units`, which copies each unit's nodes out of
  the whole program's checked tables, as upstream's unit is an op of its
  module, never re-emitted from its source: `CheckedTables::extract`
  renumbers the subtree under the unit's statement node in the order the
  check's nodes are built, through `CheckedOp::map_operands` and
  `CheckedStmtOp::map_operands`, a struct member's unit holding its
  struct as a shell naming the member alone): its expression and
  statement nodes, each carrying its own op, type, and attributes as
  upstream's ops do and encoding only the attributes it has, behind an
  encoding mask, as an MLIR bytecode op does, and its declarations, beside the facts of the unit's
  own declaration sites. A member's struct's parameter list is not in its
  record: the importer gives it from the struct's declaration record. The
  encoding numbers each occurrence densely within the unit and the
  decoding draws a fresh identity per number, keeping each derivation
  (`token::{encoding_occurrences, decoding_occurrences}`). A package body
  enters the pipeline at HIR: the importer installs the unit's tables as
  its checked program (`CheckedUnit::program`, `CheckedProgram::installed`,
  over the package's own overload sets and constants, rebuilding no
  arena and no syntax: the installed program holds the unit's nodes alone,
  which HIR and MIR walk), lowers it (`mir::lower_package_unit`), and then inserts drops, as
  upstream's importer lowers its semantic control flow and runs its
  lifetime pass on a body it materializes.
- **A declaration's record** (`checked::package`) holds the declaration
  once, as upstream's package holds it in one op every phase of its
  importer reads. Its lowered part (`LoweredDeclaration`) is every MIR
  struct and function declaration it lowers to (its own, its methods, the
  functions nested in or lifted from it) and its callables' effect
  summaries; it holds no path-stamped syntax and decodes once per process,
  for MIR, the elaborator, and the check alike. The checker residue
  follows: the declaration's rows of the checker's tables, less what the
  lowered part says. The package build's check exports the rows
  (`checker::InterfaceExport`); once the library is lowered,
  `InterfaceExport::records` places every MIR declaration and summary in
  the record of the declaration its name belongs to
  (`DeclaringModules::declaration_of`, lifted names through
  `symbol::lifted_owner`), and strips from each struct row its parameters,
  fields, fieldwise initializer, destructors, and explicit-destruction
  obligation, from each method its signature apart from what MIR does not
  carry, and from a `def`'s type the same, after comparing each, byte for
  byte, with its MIR declaration: a difference refuses the package, so no
  two copies diverge. A compilation's check decodes a residue and joins
  it with the lowered part (`DeclarationInterface::join`).
- **The MIR declarations** are attached, never copied
  (`mir::link::link_declarations`): the importer's `MirDeclarations` holds
  its own declarations and an `ImportedDeclarations`, the shared
  `PackageDeclarations` (the declaration records' lowered parts), to
  which the importer adds nothing. `MirDeclarations::{function, structure,
  trait_declaration}` answer from the
  program's own declarations first, then the package's, so every phase
  looks a declaration up by name; only a consumer of the whole program
  (the disassembler, through `MirDeclarations::materialized`) decodes them
  all. The verifier checks the program's own declarations; the package's
  were verified when it was written.
- **A module record** (`module::package::PackageModule`) is what linking
  the module from source produced, as upstream's `createBinaryPackageState`
  reads a package's module tree: the modules its link asked to load, in
  order, and its export and namespace tables. It holds no declaration:
  an importer reads every library declaration resolved, from its
  declaration record. It is bytecode (`postcard` over
  its serde form), under the module's row in the declaration directory's
  module table, which holds its dotted name and its canonical and stamped
  source paths (`PackageModules` is built from the table), and decoded
  when a link reaches the module. The importer's link
  (`module::link_*_with_package`) replays a record in place of reading,
  parsing, and resolving the module, finding each recorded load by name,
  and adds none of the module's declarations to the program. The
  link reports the table position of each module it read from a record
  (`LinkedProgram::packaged`), whose interfaces the compilation then
  installs by position. An import
  resolves against the package's module table at the library root's
  position in the search order (`Linker::find_at_root`), as upstream's
  importer finds a module in a package: no library file is probed,
  canonicalized, or read.
- **Every program imports the package.** The driver links once, against
  the package (`Compiler::compile_linking`); a program that cannot link
  against it is an error, never a compile of the library's source, as
  upstream's importer always reads `std.mojoc`. A callable's
  transfer and call-through effect summaries ride in its declaration's
  record; the check reads one the first time a call names the callable and
  keeps it in the pass's effect map (`Checker::callee_transfer_effects`),
  so the transfer fixpoint compares every read against it.
- **A declaration's checker rows** (`checker::interface::DeclarationInterface`)
  are what the build's check resolved for one library declaration, as
  upstream's importer takes a package declaration `loadedFromBytecode`:
  its rows of the checker's tables, by linked name — a struct's or trait's
  checked signature (`StructInfo`, `TraitInfo`, a method's parametric
  origin writes included) and a struct's explicit-destruction obligation,
  an alias or module constant, its module-scope binding's type and
  callable origin facts, and the applicable functions and declared results
  a compile-time call reads. A table key belongs to the declaration it
  names, an overload's signature-qualified key or a member's
  `Owner.member` included. A module constant's residue also holds the
  constant as the build's comptime elaboration resolved it
  (`checker::ElaboratedConstant`: a folded value or `Applied` marker, or a
  generic alias's parameters and body; `comptime::elaborate_exporting`,
  `CheckContext::record_elaborated`), and a declaration's residue holds the
  module-scope compile-time facts its names bind (a bound dtype, a lifted
  constant's application, a kept compile-time value, a type alias, its
  `SIMDLength` binders). Its lowered part holds its integer constant's
  one exact value (`package::LoweredDeclaration::constants`), which the
  residue's integer row and elaborated value are joined back from, as
  upstream holds a constant once in its declaration's op; the build
  refuses a library whose check and elaboration fold a constant to other
  values (`InterfaceExport::checked_constants`,
  `DeclarationInterface::project`). In every compilation the check folds
  each module integer constant once, first, before any declaration's
  symbol is spelled (`Checker::fold_module_constants`), and every symbol
  speller — the checker's binder owners, the conformance oracle, and MIR's
  name lowering — reads that value (`checked::package::ProgramConstants`,
  `CheckedProgram::constants`); the overload scan folds nothing. It holds no overload set: as
  upstream's package holds none, an importer's name's overload set is the
  reached declarations a lookup finds under it, the plain name and its
  `$ov$` run of the by-name function table (`PackageScope`'s
  `OverloadLookup`). A `Trait.method` name's set is its trait
  declaration's requirements, every one, defaulted or not, as upstream's
  trait declares each (`TraitDeclaration::requirements`); a defaulted
  requirement's member function is its body, not another declaration of
  the name. The build refuses a library whose lowered declarations —
  functions, structs, and traits — imply other sets than its check scanned
  (`checked::declaration::declared_overloads`). No module has a checker
  record of its own. The build's check
  exports the rows once the whole library is checked
  (`Checker::export_interfaces`); the declaration directory lists each
  module by path, and each declaration's record with every table key it
  holds and, for a struct, every method it declares. A residue holds no binding owner, span-keyed
  fact, syntax identity, or parameter hole, and encoding a table that
  holds one fails the build; a development build then compiles the library
  from source, after one warning.
  It is bytecode (`postcard`); a default argument or a conformance
  condition in it stays AST, stamped on decoding with the path the
  importer reached the module by (`ast::decoding_from`).
- **A declaration resolves on first lookup**, as upstream's importer
  resolves a package declaration from bytecode
  (`lookupAndResolveMangledDecl`, `resolveDeclFromBytecode`). The process
  indexes the declaration directory once (`checker::DeclarationInterfaces`); each
  compilation marks the modules its link reached and decodes no record
  (`checker::InstalledInterfaces::reaching`). The
  checker's declaration-keyed tables (`checker::DeclarationTable`,
  `DeclarationSet`) hold the program's own entries over that handle: a
  lookup the own entries miss finds its key's declaration in the
  directory and decodes that residue the first time, once per compilation,
  for every checker pass and the conformance oracle alike; a key list or a
  name-set membership answers from the directory alone. A module-scope
  compile-time fact the checker's own scopes miss is the installed
  declaration's of that name (`Checker::installed_comptime_parameter` and
  its siblings), below the module scope. A package module
  binding resolves below the module scope, with an owner from a block each
  pass reserves for every reached binding, numbered as the modules were
  reached and then by name (`Checker::installed_binding`). Only a whole-
  table walk decodes every record of its table, and the check makes none
  on the importer path. A record that does not decode is a lookup
  miss and fails the compilation that reached it, never another: its
  verdict lives in the record's own cell, decoded once per process
  (`PackageDeclarations::decode`), and each compilation's view of the
  package notes the failed records its lookups reach (`ReachedFailures`).
  The check's view (`Installed`, its `PackageScope`, and the effect
  lookups) is refused after the check (`InstalledInterfaces::failures`);
  MIR's (`ImportedDeclarations`) is a program verifier finding
  (`mir::verify`), and the elaborator's reach through it is
  `MonoErrorKind::Decode`, which outranks the miss's own symptom. A
  package body's materialization has a view of its own: its unit's
  lowering, its finish, and its verify each note what they reach, into
  that body's verdict (`PackageBodies::reached`, led into
  `PackageBodies::findings`), and the elaborator refuses a body whose
  verdict names one as `MonoErrorKind::Decode`, naming the body.
- **Above the waist no package body or declaration is prepared,
  elaborated, or checked again.** The driver partitions the link into the
  package's traits, borrowed, and the program's own statements
  (`Compiler::compile_importing`). `comptime::prepare_importing` rewrites
  the own statements alone, reading the package's traits, which a
  conformer inherits defaults from, and its SIMD, `!kgen.type`, and `!kgen.string` aliases
  as its build elaborated them. `comptime::elaborate_importing` elaborates them alone
  (`comptime::ImportedScope`): a name its module environment
  (`comptime::ConstEnv`), generic aliases, or module type aliases miss is
  the reached package's constant of that name, decoded with its
  declaration's record on first lookup
  (`checker::InstalledInterfaces::elaborated`), and its conformance oracle
  attaches the interfaces and registers the own statements over them
  (`ConformanceOracle::importing`). Its struct and
  function indexes hold the own declarations; a package struct or `def`
  is read from the oracle as the package's check resolved it
  (`ConformanceOracle::{package_struct, package_associated_value,
  package_field_types, package_function_params}`), as upstream's emitters
  read a package declaration resolved, never its syntax.
  The check is given the program's own statements alone: each transfer
  pass attaches the declaration records of the modules the link reached
  to its fresh `Checker` (`Checker::attach_interfaces`), copying nothing
  in, then scans and checks the own statements over them, taking a
  package callable's summaries as committed effects. The
  checked arena, explicit destruction, and lowering therefore see the
  program's own statements alone.
- **Lowering** (`mir::lower_checked_program_importing`) lowers the checked
  statements, reads the checker's overload sets
  (`CheckedProgram::overloads`: a shared `package::ProgramOverloads`, which
  asks the program's own sets and then the reached package declaration
  owning the name, `package::PackageScope`), and links the
  package's MIR declarations (`mir::link`) before closing and verifying the
  program's own functions. The check states conformance facts for the
  program's own structs and traits alone (`Checker::conformance_facts`),
  and answers no struct for a trait ahead of use: a package struct's
  conformance to any trait an importer's clause asks is decided by the
  elaborator from the struct's declared conformances and the trait
  declarations' parents, as upstream decides a conformance when a clause
  asks it.
- **The elaborator** (`native::mono::specialize_importing`) reads a callee
  the program does not define from the package. The body is lowered from
  its unit, drop-elaborated, and verified when first reached. Upstream's
  precompiled package likewise holds each body as checked, before its
  semantic control flow is lowered and its lifetime pass inserts
  destructors (the checks run at package build time but keep no rewrite),
  and its importer runs those passes on each body it materializes
  (`docs/notes/upstream-package-contents.md`).
- **Identity.** A package names each library file relative to the
  library's root (`std/collections/list.mojo`), as upstream's names
  `Mojo/stdlib/std/…`, and its declarations by linked name; the importer
  joins its own root back on when it reads the module table and decodes a
  unit (`checked_package::LibraryRoot`), and the build refuses a package
  whose bytes name the root it was built under, so a package reads the
  same wherever it is installed. The build id (`MOJITO_BUILD_ID`, a digest
  of the compiler's sources and the bundled library's source, stamped by
  the root `build.rs`) alone keys it, so the importer reads no library
  source to find its package, as upstream's imports its installed
  `std.mojoc`. An installed compiler (`module::installed_prefix`) reads
  `<prefix>/lib/mojito/std.mojoc`, which `mojito precompile` writes and
  `scripts/package-pliron` ships, and refuses to compile when it is
  missing or another build's, as upstream's does; a development build
  writes its package to `target/mojito-cache` the first time it needs it,
  as upstream's build system builds `std.mojoc`. Editing the library takes a compiler rebuild,
  which every `cargo` command does, as an upstream toolchain keeps its
  package until the library is rebuilt. An installed bundle ships the
  package and no library source, as upstream's ships no `std` source:
  `module::installed_prefix` finds the bundle by its `share/mojito/`
  directory, and its library root names where the package's files are,
  not a directory that exists. `MOJITO_STD_PACKAGE=0` and
  `Compiler::with_std_package(false)` compile the library from source in a
  development build only; an installed compiler refuses them, and refuses
  to `precompile`.
- **Divergences.** Upstream's package holds each body as typed `lit` ops;
  Mojito's holds the body's syntax beside one record of the check's facts
  per occurrence, its checked handoff's own shape, and those facts still
  name the package check's binding owners (`docs/roadmap.md`,
  `mojo-shape` track: R616, R611). A module record holds no
  declaration: an importer reads every library declaration, trait
  defaults included, from its declaration record. A declaration's overload
  facts restate what its lowered functions imply (`pmir` track).

`native::mono` is the one elaborator below the waist. `concrete_mir()` runs it
from the program's entry roots and caches the graph, so the VM and the native
backend read one elaboration and no backend elaborates again.
`mir::ConcreteMir` has one constructor, which runs concrete verification, so
a holder never has to know which verifier mode ran. A body the elaborator
refuses is `CompilerError::Elaborate`; there is no other execution path.
`concrete_mir()` refuses a program whose own bodies carry verifier findings
(`CompilerError::Verify`), for every backend, and the elaborator refuses a
package body it materializes whose finish or verify left findings
(`PackageBodies::findings`, `MonoErrorKind::Verify`). The verdict is the
body's own, so a body no compilation reaches fails none, as upstream's
importer fails only the compilation that materializes a bad body. A package
declaration that does not decode is judged the same way: the elaborator
refuses one its lookups reached (`MonoErrorKind::Decode`), or one a package
body it materializes reached while it was lowered or finished
(`PackageBodies::reached`), and only
`drop_elaborated_mir`, whose artifact holds the whole package, reports every
record that does not decode (`PackageDeclarations::failures`).

The entry roots are defined once, by `mir::entry_roots`: `main` and
the module initializer `__toplevel__`, each when the program defines it.
Source execution, artifact execution, and a native executable all elaborate
from them. A native caller that wants another root (a scalar entry under
test) names it to `mono::specialize`, through `pliron::compile_mir`.

`Compiler::execute` runs concrete MIR through `Backend::run_concrete`, the
VM's one executable entry: the VM interprets concrete MIR and nothing else,
and a parametric instruction, terminator, or constant reaching it is a
compiler invariant error, since `verify_concrete` rejects each before a
program runs. Compile-time evaluation runs concrete fragments the elaborator
builds (`VmBackend::call_concrete`). The corpus binary's `artifact` group
compares the production run with the serialized artifact's run per fixture.

`CompiledProgram::emit_mir` serializes the drop-elaborated program to
canonical text. Post-drop verification findings fold into its
`invariant_errors`, which every consumer refuses when non-empty. Production
backends therefore never re-lower checked syntax. `CompilerError` identifies
the failing stage. Individual stage functions remain public seams for phase
tests and diagnostic tools; every executing test goes through `Compiler`
(a generic body forwarding its own `H`
into `hash[H](x)` binds the declaration default there, while `Compiler` binds
the caller's).

The design is an hourglass:

```text
parsed AST
   |
   v
module linker
   |
   v
source validation (every comptime arm, parameters symbolic)
   |
   v
comptime elaborator
   |
   v
semantic checker
   |
   v
HIR CFG
   |
   v
MIR  <---- stable waist
   |
   v
analysis + drop elaboration
   |
   v
elaborator (native::mono): concrete MIR
   |
   v
register VM / native backend
```

The MIR is the important waist. Earlier phases preserve source structure,
perform declaration-time rewrites, and protect later phases from unsupported
syntax. Later phases should consume verified MIR rather than rediscover language
semantics from the AST.

## Design Goals

The architecture prioritizes:

- correct subset semantics over raw speed
- explicit control flow before ownership analysis
- explicit places before borrowing and partial moves
- deterministic ASAP destruction
- clean rejection of unsupported constructs
- a small compiler that is still recognizable as a systems-language
  implementation

mojito does not today reproduce Mojo's production architecture, and that
is now regarded as a mistake that must be corrected (`docs/pliron-future.md`).
Resembling Mojo's own implementation as closely as a small compiler can is a
goal, pursued in stages over a long horizon
(`docs/parametric-mir-plan.md` stages the pipeline order); the arrangements
described in this document are where the code stands, not an argument that
the distance from Mojo is right. Whether MLIR — or any backend IR — becomes the compiler's
internal layer is one of those staged questions
(`docs/pliron-backend-pivot-plan.md`), no longer a standing exclusion.

What the current pass does not attempt is scope, not architecture: it targets
single-threaded CPU language semantics and leaves out parallelism, distributed
execution, Python interoperability, legacy `fn`/`owned` and other removed
source spellings beyond clear rejection diagnostics, and escaping closures with
the removed `escaping` effect — closure parity targets Mojo's current
non-escaping capture-list model. Two of those lines are drawn differently.
Concurrency in its most primitive form is expected: not tasks or parallel
execution, but the smallest primitives the language needs, on no schedule yet.
GPU is a stretch goal: we intend to reach it, but it is not on the immediate
horizon. Distributed execution and Python interoperability are the ones we do
not intend to pursue. The register VM is
the executable specification. A versioned textual MIR/VM assembly form is the
next representation boundary; the prioritized native backends are the
Rust-native, MLIR-inspired [Pliron](https://github.com/pliron-org/pliron) (its
LLVM dialect emits LLVM IR; `docs/roadmap.md` contains the staged adoption and
fallback plan) and Cranelift, with a C or C++ source backend as a possible addition.

Native-backend work is isolated from the default build as an invariant: the
default `mojito` build and `scripts/check` resolve no LLVM or Pliron
dependency (`tests/backend_isolation_test.rs` guards the default feature
graph). The supported backend lives in the workspace crate `crates/mojito-pliron/` behind the
optional `backend-pliron` feature — a compile path
(`mojito compile --backend pliron`, plus `run --backend pliron` for the
advertised subset) consuming the cached `concrete_mir` graph,
with its own gate (`scripts/check-pliron`, which also chains the Stage 0
spike gate) and a separate memory-heavy corpus lane
(`scripts/check-pliron-heavy`, `tests/heavy/`). Production execution stays on the register VM. The current pin
and promotion rationale live in `docs/notes/pliron-promotion.md`; the per-stage designs and recorded
VM/native divergence policies in `docs/notes/pliron-stage1.md` through
`docs/notes/pliron-stage4.md`.

The backend-independent half of that work has one owner: the un-gated
`native` surface (`crates/mojito-native-core/` for target/layout/runtime-ABI
below the MIR waist, re-exported with monomorphization and mangling by
`crates/mojito-native/`) holds the checked build configuration (target triple
with a pinned data-layout string, CPU features, optimization level, output
kind), the layout engine, the injective symbol mangler, and the runtime ABI
contract table, and the workspace crate `crates/mojito-runtime` implements
the versioned `mjrt_*` C ABI every produced executable links (never
depending on the `mojito` crate or the VM's `Value`). The normative contract
— scalar semantics, layouts, calling convention, reserved symbol namespaces,
per-symbol runtime rules, and the mechanical Rust/LLVM agreement checks — is
[`docs/native-abi.md`](native-abi.md); any later backend (Cranelift
included) consumes the same module and runtime.

Native backend support and its required gates are Linux-only, currently for the
single target defined in `docs/native-abi.md`. Supporting additional hosts or
targets requires a separate, explicitly resourced decision; it is not a Pliron
promotion condition.

### Native Backend Contract

The preferred native architecture sits entirely below the verified-MIR waist:

```text
source -> Compiler -> ownership-verified, post-drop verified MIR
                            |                 |
                            |                 +-> canonical .mir artifact
                            v
                 elaboration (native::mono)
                            |
               mir::verify_concrete: concrete MIR
                            |                 |
                            |                 +-> VM
                            v
                    Pliron lowering
                            |
             Mojito ops only where justified
                            |
                    Pliron LLVM dialect
                            |
            LLVM IR / bitcode / object / executable
                            |
                    versioned runtime ABI
```

Pliron does not replace Mojito MIR in the current architecture: it is an
optional lowering, verification, transformation, and optimization framework
below the serialized MIR handoff. Making it a required IR framework is an
open, staged decision (`docs/pliron-future.md`,
`docs/pliron-backend-pivot-plan.md`), and until a stage lands the contract
here is the rule. A backend consumes `MirProgram`; it does not import AST,
HIR, checker, ownership, or call-binding policy. The native interface accepts a
`&MirProgram`, a target description, and an output kind, and returns textual
IR, bitcode, an object, or an executable plus structured diagnostics;
`run --backend pliron` executes only the advertised subset natively and
rejects everything else with a contextual diagnostic — never a silent VM
fallback. Origin and ownership facts erase after validation, while explicit
drop and cleanup instructions remain executable behavior. Errors and
`try`/`finally` lower as tagged outcomes and explicit CFG edges; platform
unwinding stays out until it has its own semantic, ABI, and portability
specification.

Variant storage lowering (the `__VariantStorage` primitive behind the
self-hosted `std.utils.Variant`) uses the shared native tag/payload layout
and dispatches owning operations and destruction from the runtime tag. Multi-lane SIMD is
stored as a contiguous lane-aligned scalar aggregate and computed as LLVM
fixed vectors in SSA between loads and stores; the vector representation is
backend-private, and the VM's lane semantics remain the contract. User `__moveinit__` and `__deinit__` bodies remain
ordinary compiled MIR calls, with a `deinit self` callee owning residual-field
teardown.

Optimization policy lives in one profile-to-pipeline table
(`backend/pliron/pipeline.rs`, snapshot-pinned): every profile runs the same
pliron cleanup stage between whole-module verifications, and only `release`
adds the pinned external LLVM pipeline. External tools are resolved and
version-checked up front as a reportable toolchain object; the runtime
archive validates by provenance, digest, and its embedded ABI version before
linking. Emission is deterministic and failure-atomic: clean builds are
byte-reproducible at both profiles, emitted objects carry a validating link
manifest consumed by the CLI `link` verb, and executables default to DWARF
line-level debug information attached below the conversion boundary. The
policy record is `docs/notes/pliron-stage6.md`.

Dialect policy: lower directly to Pliron's LLVM dialect, and introduce a
narrow `mojito` Pliron dialect only for demonstrated needs such as runtime
calls, checked traps, explicit error propagation, target-independent
aggregate constants, or lifecycle normalization. Do not reproduce the MIR
schema as a second operation set. That policy governs the backend as it
exists. The `mojito.semantic`/`mojito.core`/`mojito.abi` dialects in
`docs/pliron-backend-pivot-plan.md` would supersede it, and only a landed
migration stage does so. Every custom operation needs textual syntax, a
verifier, negative coverage, and a total conversion rule; LLVM
emission rejects any residual illegal operation. Missing Pliron facilities
are upstreamed as narrowly scoped patches rather than accumulating an
untracked local fork.

The front end's parameter-expression layer (`mojito_types::param_expr`, Stage
2 *Parameter Expressions*) is independent Rust data shaped like a dialect's
uniqued typed attributes, so moving it into a parametric dialect is a
re-homing of storage and not a redesign. It needs no Pliron dependency, and it
does not move `CheckedProgram`, the MIR waist, or the VM's authority.

Every backend stage is tested through four complementary layers:

1. IR unit tests for each lowering, verifier, rewrite, conversion, ABI type,
   and unsupported diagnostic.
2. Canonical Pliron/LLVM snapshots with UTF-8, LF, and one trailing newline.
3. VM/native differential tests comparing stdout bytes, bindings or result,
   error category, and ordered lifecycle events at `O0` and optimized levels.
4. Artifact tests proving `.mir -> VM` and `.mir -> native` consume the same
   serialized program.

Track compile time, peak memory, IR size at each boundary, object size,
execution time, and supported/excluded MIR counts (the generated
`conformance/pliron-parity.tsv` and `conformance/pliron-capability.tsv`
manifests). Every stage is removable by disabling its optional feature and
backend modules without changing MIR, VM, or source semantics. The default
response to a correctness failure is to disable the offending optimization or
native path until parity is restored. Upstream or distribution risk can justify
the Cranelift alternate recorded in `docs/roadmap.md`, never erosion of the
MIR/VM contracts.

### Source Module Boundaries

Large phases keep orchestration in their root module and delegate reusable
policy or data-model responsibilities to focused children:

- `call.rs` is the phase-neutral function-call contract: it normalizes parser
  marker indexes and matches positional, keyword, default, `*args`, and
  `var **kwargs` collector inputs to parameter slots. The checker and VM separately adapt its
  structural errors and matched slots to types and runtime values. A
  compile-time bracket list binds through its `match_parameter_slots`, as
  Mojo binds one: positional arguments first, each to the next parameter
  that is not infer-only (a pack takes the rest), then each keyword by
  name, an infer-only binder included; the checker's `bracket_bindings`,
  `bracket_argument_slots`, and `split_callable_specialization` all use it.
- `checker.rs` holds the `Checker` state, constructors, and shared prelude
  types, and coordinates checking; the single `Checker` type's methods are split
  by responsibility across `impl Checker` blocks in the `checker/` submodules —
  `statements`, `inference`, `indexing`, `method_calls`, `call_inference`,
  `type_resolution`, `traits`, `origins`, `scopes`, `constraints`, `operators`,
  `iteration`, plus the earlier `annotations`, `builtins`, `calls`,
  `declarations`, `generics`, and `places`; `conformance`,
  `overload_support`, and `traits_support` hold the extracted conformance
  oracle and free helpers, and `method_calls`/`origins` are themselves
  directory modules split by responsibility. See `docs/symbol-map.md` for
  the per-file responsibility map.
- `mir/mod.rs` lowers ordinary code; `mir/ir.rs` defines the MIR data model and
  `mir/nested.rs` owns capture analysis and nested-function lifting. A
  lifted function is a generator over its enclosing declarations' binders:
  its declaration names the declaration it is nested in
  (`MirFunctionDeclaration::enclosing`), whose binders the verifier's scope
  admits in its body, and `native::mono` instantiates it under each instance
  of that declaration (`Specializer::instantiate_nested_bodies`). One
  declared inside a kept `comptime for` is a generator over the loop's
  index too (`MirFunctionDeclaration::region_binders`), instantiated once
  per unrolled copy of the loop (`instantiate_region_references`). A call
  of a nested `def` by name is a direct `call` of the lifted function
  passing the captured places as its leading reference arguments
  (`Flatten::lower_nested_call`), as the pin calls the lifted closure
  generator. A body with a copied or moved capture and no use but calls
  by name holds its environment in its declaring frame
  (`NestedInfo::holds_environment`): the declaration copies or moves each
  such capture into a slot of its own, and a call passes that slot, the
  shape upstream's SROA leaves of the closure's `__storage` struct. A
  closure (`closure.make`) is made at a first-class use, or at the
  declaration when a capture is copied or moved and the body has another
  use, or a sibling captures the closure itself (`NestedInfo::slotted`),
  and only such a slotted body with a copied or moved capture is called
  through its closure value.
- `backend/vm.rs` drives execution; `backend/vm/calls.rs` owns runtime argument
  binding and construction, while `backend/vm/places.rs` owns projected storage
  navigation and access; further `impl VmBackend` clusters live in
  `backend/vm/{values,adapters,invoke,dispatch}.rs`.
- `comptime.rs` runs elaboration and specialization;
  `comptime/rewrite.rs` owns AST substitution and value materialization, and
  the root's extracted clusters live in
  `comptime/{elab,synth,packs,params,requests,crossing}.rs`;
  the lane-width scan it shares with the checker lives in
  `mojito-ast/simd_width.rs`.

Child modules expose only the phase-internal operations their coordinator needs.
The public entry points for each compiler phase remain in its root module.

## Stage 1: Module Linking

Module:

```rust
crates/mojito-module/src/module.rs
```

Entry points:

```rust
module::link_with_options(entry_path, options) -> Result<Vec<Stmt>, ModuleError>
module::link_source_with_options(source, entry_path, options) -> Result<Vec<Stmt>, ModuleError>
```

`link` and `link_source` are convenience wrappers using default options.

The module linker is deliberately small. It consumes parsed source plus an entry
path and returns one flat `Vec<Stmt>` for the rest of the compiler.

Currently it supports:

- `from module import Name, Other`
- `from module import *`
- dotted module paths such as `from collections.list import List`
- relative imports such as `from .optional import Optional`
- qualified `import module as alias` member access and selective member aliases
- unaliased dotted namespaces, qualified exported types, and lexical block imports
- dots-only relative sibling imports (`from . import sibling`)
- source packages identified by `__init__.mojo`, including package re-exports
- ordinary namespace directories, every dotted-prefix binding, and lexical
  shadowing of an imported namespace tree
- source-package precedence over a same-named source module in each search root
- underscore-prefixed wildcard privacy and collision-free module identities
- transitive imports, dependency-first hoisting, deduplication, and simple cycle
  breaking by canonical path

Imported declarations receive module-qualified internal names before they are
hoisted. A module under a search root is named by its path under that root
(`std/io/file.mojo` is `std.io.file`), whichever import first reaches it, so a
declaration's linked name is the same in every program, as a compiled
package needs. Import bindings are rewritten to those names, so two modules can export
the same source name without merging declarations or overload sets. A module
exports top-level `def`, `struct`, `trait`, and `comptime` declarations except
`main`; wildcard imports omit underscore-prefixed names.

The entry module's declarations keep their source names, except that one
whose name a loaded module links bare (the bundled publics
`implicit_public_identity` keeps unqualified, such as `List` or `range`),
a compiler builtin identity (`builtin_module_exports`, such as `Slice`, or
an import-only one, `qualified_builtin_exports`, such as `ContiguousSlice`), or
a prelude name (such as `String`) takes the entry module's qualified name
(`module::entry_item_name`, module `__main__`), as every Mojo declaration
lives in its module's namespace: the entry's `struct List` shadows the
prelude's in the entry and never meets the library's, nor a spelling the
compiler recognizes the library's by. An
imported module named `__main__` would share that prefix; nothing rejects
it. A name the entry imports explicitly cannot be declared again
(`ModuleError::Redefinition`), as the pin rejects it.

An absolute import of `std.*` resolves at the bundled library's root alone
(the package's module table, or a development build's library source),
never beside the importer or under a search root, as upstream's toolchain
always reads `std` from the package it ships.

Each lexical import scope separately records names established by explicit
imports. Repeating the same target is idempotent and an explicit import may
shadow an implicit prelude binding, but a different target cannot overwrite an
already explicit local name. Import resolution also compares canonical source
paths and rejects an exact self-import. A module publishes provisional local
exports before following dependencies, so this check does not mistake a real
two-module cycle for self-import and mutually recursive modules can bind each
other's declarations deterministically.

Package directories resolve through `__init__.mojo`. Imports in that file become
package re-exports. Mojo forbids executable file-scope statements, so package
initialization means declaration and compile-time initialization rather than
Python-style runtime import side effects.

Submodules are not implicitly visible to siblings or to a package namespace.
They become visible only through an explicit import or an initializer re-export.
The source half of nightly lookup is therefore deterministic. Versioned `.mojoc`
and legacy `.mojopkg` artifacts are intentionally deferred to the artifact
loader; once present, the complete per-directory order is source package,
`.mojoc`, source module, then `.mojopkg`.

Linking retains flat name-binding semantics, but recursively stamps every
statement and expression with its source module path. The file path and local
byte range remain diagnostic provenance. After compile-time elaboration and the
checker's final trait-default cloning pass, one exhaustive walk
uniqueness-normalizes each concrete statement/expression `SyntaxId`: an
already-unique input identity is retained, while every repeated clone receives a
new occurrence identity before semantic facts are recorded. The transitional
`SourceSpan` key carries that optional occurrence discriminator for checker/HIR
lookups, but provenance comparison and MIR source maps deliberately ignore it.
Thus two elaborated clones may point at the same source text without sharing a
type, binding, overload, effect, or declaration fact. Compile-time rewriting
still re-stamps generated subtrees from their owning declaration for diagnostics;
source stamping is no longer relied on as semantic identity.

## Stage 2: Comptime Elaboration

Module:

```rust
crates/mojito-comptime/src/comptime.rs
```

Entry points:

```rust
comptime::prepare(program: Vec<Stmt>) -> Result<Vec<Stmt>, ComptimeError>
comptime::prepare_importing(own: Vec<Stmt>, traits: &[&Stmt], installed: Option<&InstalledInterfaces>) -> Result<Vec<Stmt>, ComptimeError>
comptime::elaborate_prepared(prepared: &[Stmt]) -> Result<Vec<Stmt>, ComptimeError>
comptime::elaborate_importing(own: &[Stmt], imported: ImportedScope) -> Result<Vec<Stmt>, ComptimeError>
comptime::elaborate_exporting(prepared: &[Stmt]) -> Result<(Vec<Stmt>, ElaboratedConstants), ComptimeError>
comptime::elaborate(program: Vec<Stmt>) -> Result<Vec<Stmt>, ComptimeError>
```

`comptime` is implemented as a phase distinction before type checking. The
elaborator rewrites compile-time constructs into ordinary AST so the checker,
HIR, MIR, and VM do not need to carry special semantics for most of them. A
`comptime if` and a `comptime for` in a generic `def`'s body are the
exceptions, the constructs that cross the waist as themselves. The template
keeps a `comptime if`, the checker types every arm with the binders symbolic
and records each condition it compiles as a constraint
(`SemanticAdjustment::ComptimeCondition`), HIR and MIR lower it as the
`ComptimeIf` region op (`hlcf.comptime.if`), every condition first, then one
region per arm, and `lower-control-flow` lowers that to a chain of
`ComptimeBranch` terminators — branches whose condition is a
`GenericConstraint` over the function's binders, not a register — and
`native::mono` decides it under the instance's bindings after drop
elaboration (`select_comptime_branches`), keeping the taken arm and pruning
the other (`mir::prune_unreachable_blocks`) before any call in it is
enqueued. A condition the constraint compiler does not close (an
application) is lowered as a zero-parameter thunk over the binders in scope at
the condition — the function's, then the enclosing `comptime for` indices,
which it reads as parameter references (`Const::Param`) — (`ComptimeThunks`,
`lower_expression_thunk`), and its branch carries the
thunk's application, which the elaborator evaluates on the VM
([`docs/notes/ctfe-request-path.md`](notes/ctfe-request-path.md)). The
template keeps every `comptime for` in a generic body, as upstream keeps its
parameter `for` (`comptime/elab.rs::keep_template_comptime_for`, which
forces a pending module constant the loop iterates): the checker types the body
once and records how the loop iterates
(`SemanticAdjustment::ComptimeIteration`). Every iterable — a collection
display, a display binding, a module constant, an application, `range(...)`,
a value pack, a reflected field-name list — is iterated by upstream's
iterator protocol (`Checker::check_comptime_iterator_for`,
`checker/comptime_iteration.rs`):
the check spells the iterable as its `__iter__()` call, which the checked
arena hands HIR in the iterable's place and MIR lifts as a thunk over the
binders in scope, the header's binder is the iterator, and its
`ComptimeSequence` steps it by the applications of the bundled
`std.builtin._stubs` functions `paramfor_has_next` and `paramfor_next_iter`
over that binder, which `native::mono` runs on the VM per step, as
upstream's `processParamForOp` runs them on its interpreter. The loop
variable is no binder and has no slot: it is a local alias of
`paramfor_next_value` over the iterator (the header's `next_value`), which
each body copy runs once
and materializes as any application alias is; a function lifted in the
body over the variable takes the iterator binder. The stubs are linked
ahead of the prelude, so a bundled module loaded while the prelude
bootstraps steps its loops by them too. An aggregate loop variable is a
compile-time parameter of the element's type, as upstream's induction
variable is: a borrow of the variable (the `ref self` of `p[0]`, a read
argument, a field read, a receiver) materializes it into a temporary
(`Checker::materialized_reference_actual`,
`Checker::materialize_parameter_read`) the ownership analysis destroys, and
`native::mono` turns every read of it, per unrolled copy, into a `Const::Value` typed by the read's
register, a `String` leaf included, as upstream's `kgen.param.constant`
materializes a parameter at each use: each backend writes the frozen
fields into fresh storage, and no initializer runs again. An element of a
nested list display is a frozen `Array` like any other struct. A tuple, struct, or vector value
parameter's slot follows the same rule: it owns nothing and is no drop root
(`mir::materialized_parameter_slots`), and each read of it is the value's
typed constant, with nothing built at entry; a place of a slot whose value
owns a buffer that survives the substitution is an error
(`reject_parameter_storage_reads`). A field chain off
either (`q.a`, `q.i.v`) is upstream's struct extract on the parameter
attribute: the checker records it as a `SemanticAdjustment::ParamValue`
(`Checker::infer_member` over `struct_value_field`), its root never read,
and `native::mono` answers the `Const::Param` per instance with the
field's constant, typed by the read's register, a `String` field
included (`answer_param_constants`). A display over the binders in a
loop header is the receiver of the spelled `__iter__()` call MIR lifts,
which `native::mono` demands, runs on the VM, and freezes
(`VmBackend::freeze`, which freezes a nominal `Tuple` to its elements, a
`String` to its text, and other structs to their fields and pointer memory),
as upstream runs the display on its interpreter per instance. A lifted
function stores each value it borrows — a local `comptime` binding, a
materialized receiver or argument temporary — in the VM's compile-time
memory (`MirInstr::StoreToMem`, upstream's `store_to_mem`), which is never
destroyed, so a result that borrows it (a `d.keys()` iterator) freezes its
reference as `CtValue::StoreToMem` rather than dangling. A local
`comptime` binding of such a display is a compile-time value with no runtime
form, whatever reads it: the check types the name, records the binding on
the display (`SemanticAdjustment::ComptimeDisplay`), steps a loop over the
name by its `__iter__()`, and rejects a read outside every
compile-time position as the pin does. The binding denotes one value in the
parameter domain, its sequence: the application of the function MIR lifts
for the display, which the check names by the binding and applies to every
binder in scope (`Checker::record_display_binding`). MIR keeps the binding,
a reference to its value's node, for the scope that declares it, as an SSA
use names its definition (`DisplayBinding`, `Flatten::displays`), and lifts the display under
that name where it is declared (`ComptimeThunks::bind_display`), and every
header over the name carries that one application. A type or a parameter
argument reads the sequence: an element by position is `param_list.get`
over it and the length is the builtin application `param_list.length`
(`Checker::display_read`), alone or under integer arithmetic, so
`SIMD[DType.int32, L[0]]`, `g[len(L)]()`, a `range` bound, and a local
`comptime` value bound to one (`comptime e = L[1]`, then `g[e]()`) are
parameter expressions the template carries. Two reads are one expression by
structure and none is the element the display spells, as at the pin, which
keeps `L[0]` distinct from `n`. `native::mono` runs the function when a
substitution meets its application at an instance's arguments
(`Applications`, `Specializer::answer_pending`) and folds what stands above
it. Every other compile-time read is lifted where it
stands, as upstream runs it on its interpreter: a `comptime
if` condition as any uncompiled condition is, a `range` iterable (the
thunk of its spelled `__iter__()` call), and an `Int` or `Bool` a
runtime position, another binding, or a crossing reads (`print(L[0])`,
`comptime(len(L))`), which
MIR carries as a parameter constant holding the application
(`Flatten::display_read`). Each thunk begins with the statements of the
local `comptime` bindings its expression reads that denote no parameter
expression — the displays, and a value such as `comptime t = (L[0], L[1])`
(`ComptimeThunks::bind_evaluated`, `thunk_prologue`) — and `native::mono` demands an
application wherever it evaluates one — a condition, a loop header, a
parameter constant, a bracket argument's expression
(`Specializer::applied`). `materialize[L]()`, and a value
read off the display that is no `Int` or `Bool`, build the display where
they cross to runtime (`Flatten::crossing_display`, `build_display`). An `Int` or a `Bool` that a type or a parameter argument, or a local
`comptime` value, computes some way the parameter domain does not express
(`g[h(L[0])]()`, `flag[n > 2]()`, `SIMD[DType.int32, h(n)]`, `comptime e =
h(n)`) is an application too, of a function the check names for the
expression (`Checker::lifted_application`, from `eval_associated_ct` and
`compile_dependent_ct_expr` inside a `lifting_position`, recorded as
`SemanticAdjustment::ComptimeApplication`): the function is applied to the
binders the expression reads (`binders_read`), two occurrences of one
expression over the same bindings share it, as the pin identifies them by
structure, and arithmetic over applications stays arithmetic. MIR lifts each
such function once the body that spells its expression is lowered
(`ComptimeThunks::request_applications`, which walks the body's checked
nodes with `CheckedTables::walk` and skips a nested declaration's body),
over the narrowest scope that
declares its binders, and reads the expression as that application wherever
it denotes one (`Flatten::comptime_application`). The checked arena holds a
node for such an expression inside an annotation, which has none otherwise
(`type_applications`, `checked.rs`), and the op that spells the annotation
names it (`CheckedOp::TypeValue`, `CheckedParamArg::Attribute`,
`CheckedStmtOp::VarDecl`'s `applications`), apart from its operands. A type
operand of an op is never the annotation as written: as upstream's call op
carries each bracket argument as the parameter attribute its check produced,
the check records the `TyArg` of each bracket argument that is not a
run-time value at the occurrence (`NodeFacts::parameter_attributes`) as the
binder that resolved the bracket list returns it, once: each binder answers
its list's `BracketAttributes` in source order beside what it bound
(`resolve_use_params`'s `BoundParameters`, `applied_type` for a type
application, `simd_dims`, the builtin collection and pointer types), a
partition that keeps origins back scatters the binder's answer into source
positions (`ForwardedBrackets::scatter`, for the struct origin partition and
an explicit specialization's), and the route holding the occurrence records
it (`Checker::record_bracket_attributes`). A type-shaped argument's
attribute is what it bound to — a type, a value it names (`Self.n`, a pack
spread `*vs`), or an origin — and a value-shaped one's is the type or origin
it bound to (`List[String]`, `Pointer[Int, origin_of(x)]`, `size_of[T]`),
which `resolve_param_arg` and origin binding also record at the argument
(`NodeFacts::argument_attribute`); a type in value position records its
`Ty` (`NodeFacts::type_operand`, `Checker::type_value`); the op holds them,
and MIR reads them instead of the spelling. A closed one in a plain `def`
(`g[h(3)]()`) is lifted the same way, over no binder. A call of a
module `def` or a static method over `Int` and `Bool` values lifts nothing:
it is the application of that callable to the parameter expressions of its
arguments (`Checker::called_application`, over the `applicable_functions`
collected before any declaration is checked), as the pin's call node is.
The application names the MIR declaration of the one overload the
arguments select, and lists the callee's own compile-time arguments, then
its runtime ones in declared order with a declared default where the call
omits one, so `d(1)` and `d(1, 3)` are one node. A static method of a
generic struct is applied first to the instance it is called on
(`G[n].w()`, `Self.w()`), a type that is still a pattern. The application
names no declaration's binders, so a signature and a struct field type take
it, a caller that binds the parameters spells the same value (`h(2)` for
`h(n)`), and inference solves a parameter through it argument by argument,
through the instance too (`solve_applied_args`, `checker/generics.rs`).
`not` and a conditional expression are the parameter domain's own
operators (`eval_associated_ct`), never a function. A raising callee is
rejected where it is called. `native::mono` runs the
function on the argument values (`Specializer::demand_application`, whose
leading argument is the owner instance of a generic struct's static method
and whose trailing arguments are the callee's runtime parameters), closes
the instance's values before it answers nested applications
(`Specializer::applied`), and demands the applications a struct instance's
field types carry (`discover_structs`). A struct whose own static method is
being run is declared by the body that uses it, once the method has run, so
a field type may apply a static method of its own struct. A display
binding is served by syntax alone
(`mojito_ast::visit::display_bindings`, `served_display_bindings`).
Evaluating such a binding again per reader is roadmap
R378, where upstream evaluates a `comptime`
alias. A thunk reads a local `comptime` value bound before it (`comptime k
= n + 1`) as the parameter expression it denotes
(`EnclosingBinders::comptime_bindings`). HIR lowers the
loop as an `HirInstr::ComptimeFor` region op, MIR lowering as the
`ComptimeFor` region op and `lower-control-flow` as the `ComptimeFor`
terminator — the binder, the slot the body reads it through, and the
sequence, as upstream's `hlcf.comptime.for` iterates a compile-time sequence —
the ownership analysis and drop
elaboration decide it as the loop of the same shape with its trip count
unknown, and `native::mono` unrolls it under the instance's bindings before
substitution (`unroll_comptime_loops`): the body — the blocks its entry
dominates — is copied once per element with fresh registers, the binder's
reads folded and its types substituted, the loops nested in the copy
unrolled under its binding first and the `comptime if`s over it decided, the
copies chained where the loop stood, and the original body left unreachable
for the pruning. An alias of a local display binding (`comptime A = L`) is
another name of it: the check binds the alias to the display's owner and
sequence, so a loop over it is the display's loop. Every other `comptime
for` — over a display with an element that applies a function, and every
one outside a generic `def` or a generic struct's method — is unrolled in
the AST as before (roadmap R363). A display element source validation typed
as a scalar is served whatever its spelling (`twice(n)`, `P(n).get()`, a
field read `P(n).twin().v`), and so is a loop-body `comptime` binding that
applies a `def` to the index (`comptime w = twice(i)`), the application
`native::mono` demands per copy. A served loop may construct a reflected
field type (`types[i]()`, `FT()` over `comptime FT = types[i]`): the check
records the construction of that type expression, MIR carries it as
`MirInstr::ConstructType`, a template-only form beside `SizeOf` and
`TypeName`, and the elaborator writes the closed type's default
construction in each unrolled copy. A type pack crosses the waist the same way: every
pack-keyed `def` keeps its body, as upstream's does
(`is_specializable_declaration` never names one, so no type-pack `def` is
cloned); the checker
types it with the collector a `VariadicPack` of the symbolic pack, each
`args[i]` the dependent `Ts[i]` (`ParamKind::ListGet`), and the pack's
length — `args.__len__()`, `Ts.length`, `len(Ts)`, as the pin reads them at
compile time — a `PackQuery::Length` the loop header carries as its bound.
A type pack is the bundled `TypeList` the pin's pack is: `Ts.length` and
`Ts[i]` are its `length` and `__getitem_param__` members applied at the
pack, which the check folds to that query and element;
MIR records the elements the checker solved for the call
(`TyArg::Val(CtValue::Tuple)` among `instantiated_args`); and `native::mono`
binds the pack whole (`bind_pack`: the `RuntimePack` every spelling of the
pack substitutes to, and the element tuple its parameter expressions fold
under), so the loop's trip count and each copy's element type close at
unrolling and the instance is named by its elements. A value pack
(`*values: Int`) is served alike when the body reads it only as its length or
an element at a compile-time index: the checker records each read as a
parameter constant over the pack's `ParamList` binder, and `native::mono`
binds the pack to the tuple of the call's values. A pack spread whole
into another served `def` or into `print` (`show(*args)`, `drain(*args^)`)
is a call whose spread argument is the collector itself, read in place or
moved, at the position `MirInstr::Call::spread` names; the ownership
analysis sees the whole collector lent or moved, and the elaborator
(`expand_pack_spreads`) replaces the argument with one place read or move
per element of the bound pack before it binds the callee, so concrete MIR
carries no spread; a spread `inline-parametric` leaves in an inlined body,
its collector already bound to its elements' tuple, is expanded the same
way (R1103). A method call carries the same position
(`MirInstr::MethodCall::spread`, schema 1.24), which the elaborator
expands. The checker types the spread against the
callee it selects, a `def`, method, or constructor whose collector is a
type pack (a method's own `*a: *Ts` or its struct's `*b: *Self.Ts`); a
spread into a homogeneous collector is rejected, as at the pin. It types a
construction (`Tuple(*args^)`, `Bag[*Ts](*args^)`) as a call of the declared `__init__` it selects, the forwarded
pack bound whole to its collector (`bind_forwarded_pack`), so MIR names the
constructor overload and its collector's ownership is checked as at the
pin. Every such call names a
declared callee: `write` resolves to the bundled `Writer` trait's default or
a conformer's own `write` (`stdlib/std/format/__init__.mojo`), and `format`
to `String.format`/`StringSpan.format` over the bundled `_FormatUtils`
(`stdlib/std/string.mojo`), as upstream's do; a string literal's `format`
is the static call of the stand-in `_FormatUtils.format_literal`
(`SemanticAdjustment::LiteralFormat`) until the literal declares methods.
A static call carries the spread too, after leading arguments or alone. A clone that spreads its bound pack into a served
`def` spells the elements itself (`PackRewriter::served_callees`). A pack query
read as a runtime value — `Ts.length`, `len(Ts)`, `Ts.contains[X]()`,
`Ts.all_conforms_to[T]()` of a `def`'s or method's own pack — is upstream's
`kgen.param.constant` with a symbolic attribute: the checker records the
query as `SemanticAdjustment::ParamValue`, MIR lowers it as the constant
`Const::Param` (schema 1.20) without lowering the pack operand, the
parametric verifier checks its binders and the concrete verifier rejects a
survivor, and `native::mono` folds it per instance
(`answer_param_constants`, after the `comptime if`s are decided): a length
through `eval_ct`, a membership or conformance through the oracle that
decides a `comptime if`. A reflection query over a parameter crosses the
same way (below). A clone binds every pack before materialization
and folds the same queries in the AST (`fold_pack_uses`). A pack element's
default construction (`Ts[i]()`, `Self.Ts[i]()`) over a symbolic pack is
`__init__` through the element's `Defaultable` witness, as `T()` is through
the parameter's: the checker records `SemanticAdjustment::ConstructPackElement`
and MIR lowers it as `ConstructTypeParam` with its `element` index (schema
1.21), so one instruction constructs a parameter or a pack element. The
elaborator's `default_construct_parameters` (`mono/substitute.rs`) writes
every nullary construction whose type the bindings decide as that type's
default construction — before unrolling for a parameter or a literal index,
and in each unrolled copy for the loop's index — and a binding of the
element under the loop takes the copy's own slot at its element type. A
type parameter has no frame slot (`callable_parameter_locals`), as Mojo
gives a type parameter no storage. A non-callable value binder's slot,
interned at its first read by `Flatten::var`, is typed by the binder's
declared type, which mono substitutes.

**One check.** `prepare` normalizes declarations
without selecting an arm, unrolling a loop, stubbing a template, or minting a
clone (pack qualification, the synthesized `copy`/`__hash__` methods, the
`SIMD[_, _]` parameter desugar into an infer-only dtype and `SIMDLength`
binder pair, SIMD alias-bound folding). The elaborator keeps every
`comptime if` and `comptime for` of every function body, plain or generic
(`Elab::def_body` pushes a binder set for every body, empty for a plain
one), and the one check types every body once with its declaration's
parameters left symbolic (`checker/comptime_validation.rs`): each condition
typed as a compile-time `Bool` (a generic constraint over the parameters
in scope, a concrete conformance, or a `Bool` value; a closed condition
folds to its verdict, `Checker::fold_closed_constraint`, and one over a
runtime local is the pin's "cannot use a dynamic value in 'comptime if'
condition"), each arm and loop body in its own scope, no arm assumed
selected — so a type error in an untaken arm rejects as upstream rejects
it, a guard such as `T == Int` narrows nothing, and an unused template
still checks. A `rebind` takes its target on faith where the operand's
type is still symbolic, exactly as upstream does; the equality is asserted
per instance by the elaborator below MIR. The explicit-destruction
analysis runs over the whole program once (`explicit_destroy::check`): an
arm whose condition names one of the body's
parameters may assume nothing about the instantiation, so such arms join as
`if` branches do: a value one arm alone destroys is abandoned, or
uninitialized if used afterwards, even when every instantiation selects that
arm, as upstream (`explicit_destroy::check_comptime_if`,
`assets/type_error/comptime_if_arm_conditional_destroy.mojo`). A condition
naming no parameter folds to one arm, and its arms join as alternatives. A
`comptime for` body must leave every outer obligation as it found it.
There is no second checker run: source validation and its tolerance for a
body it could not type (`SymbolicBoundary`) are gone, so a body with no
symbolic rule is rejected (R519).

**A variadic pack is symbolic too.** A body keyed on a pack — a `def`'s or
method's own `*Ts`, or any method of a variadic struct — is validated like any
other, whether or not it holds a `comptime` construct. The element at a
compile-time index (`args[i]`, `self.storage[i]`, `Self.Ts[i]`, a constant
index included) has the dependent type `param_list.get(Ts, i)`
(`ParamKind::ListGet` behind `DependentType::Parameter`; a `comptime for`
variable is the index's binder). That type is the element's identity: it
substitutes when `Tuple.__getitem_param__[index]` is applied at `i`, renames
when one struct's pack is forwarded as another's (`Tuple[*Self.Ts]`), and
folds to the element once the pack binds to a list
(`Checker::close_pack_elements`). Its capabilities are read through a bounded
`TyKind::Param` view (`Checker::opaque_element`): the pack's declared bounds plus
every trait a conjunctive `conforms_to(Ts.values, Trait)` or
`Ts.all_conforms_to[Trait]()` of the enclosing declarations guarantees, and
nothing else — a struct-header conditional conformance, a disjunction, and a
`comptime if` guard license nothing, as at the pin. Storage over an unbound
pack is a list that spreads it (`TyKind::Tuple`, `TyKind::Variant`, or a `Tuple[...]`
application whose one argument is the starred `TyKind::Param`,
`types::pack_spread`); a spread beside other arguments is rejected. A
`TypeList` proposition over the pack is a `Bool` no instantiation has fixed,
so both arms of `comptime if Self.Ts.contains[T]()` check, and
`__VariantStorage` operations type from `T` without deciding membership. A
variadic struct applied to concrete types inside a validated body
(`Tuple(1, "one")`, `a == b`, `t[0]`) resolves through the same signatures
with the pack bound to the element list. An explicit construction of a user
variadic struct there (`Pair[Int, Bool](1, True)`, `Bag[T, String](x^, "tail")`)
is matched against the template's constructor with the pack bound from its
`[...]` arguments (`infer_construction`), the path a bare construction takes
once its pack is solved: a `*args: *Self.Ts` collector must take exactly the
bound elements, and the fieldwise constructor its one tuple. The public
`Tuple` alone types as the tuple it spells — bare as its display, explicit
against its element list (`infer_validated_variadic_construction`) — because
its identity is the element-by-element nominal spelling, not the template's
bound pack. A call that forwards the pack whole
(`inner(*a)`, `collect(30, *items^, tail=10)`, `self.take(*a)`, `print(*a)`)
binds the callee's own pack to the caller's (`forwarded_pack_argument`,
`bind_forwarded_pack`): the spread is the last positional argument and lands
alone in a collector that is itself a pack (`call.rs:spread_position`,
`bind_spread`), every bound the callee's pack declares must hold of the
caller's, the two collectors' ownership must agree (`var` needs the `^`, a
read pack cannot be transferred), and a result naming the callee's elements
closes over the caller's pack. Where there is still no symbolic rule — a
method other than `__len__` on the pack, a spread outside a call argument —
the checker raises `TypeError::SymbolicBoundary`, which rejects the body:
there is no per-instantiation check to leave it to (R519). `docs/notes/param-expr-attributes.md` records the design.

**A SIMD lane is symbolic too.** A body keyed on a `DType` parameter, or on a
vector width naming its own parameter (`def total[dt: DType, width:
Int](v: SIMD[dt, width])`, a struct field `SIMD[Self.dt, Self.width]`), is
validated like any other. `TyKind::Simd` holds typed slots (`SimdDtype`,
`SimdWidth`): a slot is a known dtype or width, or the parameter expression
the template names (`dtype_from_arg`, `simd_width` in the checker), and the
one constructor of a symbolic vector type (`types::simd_ty_from_slots`) folds
a closed slot to its constant and two known slots to the canonical type, so
slot equality is type identity in the pin's normal form (`SIMD[dt, n + 1]`
is `SIMD[dt, 1 + n]`; `v.join(v)` is `SIMD[dt, 2 * width]`; a guard narrows
nothing). A symbolic lane's capabilities are the SIMD surface itself, not a
trait bound: every dtype-gated operator and method is licensed through
`SimdDtype::licenses` (the constraint is the instantiation's to check, as
upstream defers `constrained[...]`), an integer or float literal splats into
it where a concrete scalar does not (`types::splats_to`), and a lane fact the
checker records for lowering (a construction's dimensions, `SimdLength`,
`DtypeConstant`, a cast or `to_bits` adjustment) carries the slots
themselves, known or symbolic; a shuffle, slice, or join is recorded only at
a known width, since its mask is the width's. Applying a `DType`-keyed struct
in a validated body closes its `Scalar[Self.dt]` members under the
application's values, to a concrete lane or to the caller's own symbolic
`dt`. A symbolic slot crosses the MIR waist as a register type and in
`MakeSimd`, `SimdCast`, and `SimdBitcast` (schema 1.18): the parametric
verifier checks its binders and kind, the concrete verifier rejects it, and
the elaborator closes it per instance as it closes a register type. A
`DType`- or lane-keyed `def`, overloaded or not, is an ordinary generic `def`
served by its template unless its body keys a clone, its binders bound from
the call's arguments, an argument's lane slots, or the checker's inferred
instantiation (`n` of `a: Box[n]`), a local `comptime` binding of its
binder (`comptime lane = dt`) an alias of the binder's parameter expression. A struct
keyed on a `DType`, a lane width, a vector value, or a struct-typed value is
an ordinary generic struct, a generator: the executable check types its
members with the binder symbolic, its template crosses MIR, and
`native::mono` mints each instance, folding a read of a closed vector,
struct, or tuple binder to a `Const::Value` and a field of a struct-typed one
(`ParamKind::Field`) to its constant. A `Const::Value` tuple is the nominal
`Tuple` its register's checked type names, over the private pack storage
(the VM's `thaw`, Pliron's `store_parameter_value`);
a tuple-typed value parameter (`g[p: Tuple[Int, Int]]`) reads the same way.
A tuple or struct value whose leaves include a `String`
(`CtValue::is_constructed_parameter_value`) is the same typed constant
(`CtValue::is_materialized_constant`): each read materializes a fresh
value its consumer owns. The elaborator above the check freezes a computed
struct-typed argument (`Extent.square(4)`) to its fieldwise construction
(`Elab::freeze_struct_value_arguments`), which the checker reads as the
frozen value. Every variadic struct is a generator too, `native::mono`
binding its pack per instance. The public `Tuple`, `TString`, and `Variant`
are spelled element by element (`types::binds_pack_elementwise`,
`types::canonical_pack_arguments`): `t[k]` is the template's
`__getitem_param__[k]`, and `Tuple`'s consuming members move elements out
through pointers and end the storage with `MirInstr::MarkDestroyed`.
`Variant`'s storage operations select upstream's
`_get_type_index[T, *Ts]()` (`PackQuery::IndexOf`): the checker records it
as a `types::VariantIndex` in the operation's adjustment, HIR and MIR carry
it (`MakeVariant` through `VariantReplace`, `Proj::Variant`), and
`native::mono` closes it per instance (`close_variant_index`), failing the
instance at a `T` the pack lacks, as upstream's `Variant._check[T]()` does.
Ownership keys a symbolic alternative as one that may be any
(`Key::Variant(None)`, `VariantIndex::may_equal`), and concrete MIR holds
only known ones (`verify_concrete`). A method with a lane binder of its
own is a generator too, instantiated per call.

**A reflected field is symbolic too.** A body reading `reflect[T]` over a
parameter — a `def`'s type parameter, or `Self` in a generic struct's method —
is validated like any other (`checker/reflection.rs`). A query over a
registered struct, at concrete or symbolic arguments, is answered from the
struct table as the elaborator answers it; a query over a subject that is
still a parameter is a `ParamKind::Reflect` node: `field_count()`,
`field_index[name]()`, and `len(field_names())` are compile-time `Int`s,
`is_struct()` a `Bool`, `field_names()[i]` a string literal, and a field
type — `types[i]`, `r.field_at[i].T`, `r.field["x"].T` — the dependent
element `ListGet` of a `field_types()` list, viewed through the same bounded
`TyKind::Param` a pack element is (`opaque_element`, now keyed by the element's
spelling). A field type has no declared bound: `T`'s own bound says nothing
about a field, so a trait use of it is rejected until proved, and a
`comptime if conforms_to(X, A & B)` condition proves exactly those traits of
exactly that operand for the arm it guards (`conformance_arm_assumptions`,
pushed on `assumed_conformances` around the arm), which is what the pin
licenses for a plain parameter, a pack element, and a field type alike; a
proof on another index, after the use, in another loop, or in an `or` proves
nothing. A value read of a query over a parameter
(`field_count()`, `is_struct()`, `field_index[name]()`, `len(field_names())`,
`field_names()[i]`) in a template-served body crosses the MIR waist as a
`Const::Param` over its `Reflect` node, as upstream's `Reflected[T]` methods
return parameter attributes (`#kgen.struct_field_types`) its elaborator
folds; `native::mono` answers it per instance from the source struct's
fields (`eval_ct` through `ParamContext::answer_reflections`), by the one
policy the checker applies to a closed subject (`ReflectQuery::answer`), and
a query with no answer fails the instantiation. A field type never crosses.

**Module constants are parameter expressions.** The elaborator folds a
closed module constant and keeps its declaration; one whose initializer
applies a callable, asks a layout, or reads such a constant is marked
(`CtMarker::Applied`) and kept as written. The check classifies every
module constant in its declaration pass and records a body's read as a
`SemanticAdjustment::ParamValue` (`Checker::module_constant_value`), as
is a read of a local `comptime` binding of an application
(`Checker::local_comptime_application`), which MIR lowers as
`Const::Param` and `native::mono` folds or demands
(`docs/notes/ctfe-request-path.md` §Today, decision D3). A local
binding's statement op holds its application
(`CheckedStmtOp::Comptime::denotes`), as upstream's alias does, so it has
no slot, and a nested `def` reads it without capturing it. A constant whose
initializer the check lifts (a display, a method call) is the application
`$comptime$<name>$module`, evaluated once, and is declared ahead of the
first body after which its initializer types (`Checker::check_module_block`);
no read spells its initializer. A function the check lifts from a body (a
local `comptime` application or display binding) is
`$comptime$<declaration>$c<k>`, numbered within the declaration whose body
holds it, beside MIR's own `$comptime$<declaration>$<k>`; both are the same in
every compilation, so the bundled package may hold one. A collection display (`[1, 2]`, `{"a": 1}`)
is such an application, closed or not
(`mojito_checker::checker::lifts_comptime_value`), since the pin constructs
one by running its initializer; so is a `Dict[...]`/`Set[...]` constructor
call, and a local binding of either is kept for the check the same way. Any other closed constant — a scalar, a
`DType`, a vector, a closed tuple or struct — is a folded attribute
(`Checker.module_values`), its read a `ParamKind::Constant` at the declared
type. The toplevel lowers no `comptime` declaration, and the elaborator
spells no module constant into a body (`mark_struct_names` only marks
struct names).

Compile-time values are represented by:

```rust
CtValue::Int
CtValue::Bool
CtValue::Str
CtValue::Tuple
CtValue::List
CtValue::Type
CtValue::Expr      // a residual parameter expression, never a constant
CtValue::Deferred  // a slot whose value arrives later; not part of identity
```

### Types

A type is a handle to shared, immutable storage, as MLIR's `Type` is a
pointer to storage its context uniques (`mojito_types::types`):

- `Ty` is one pointer to a `TyNode`, which holds the variant (`TyKind`)
  and two facts cached when the node is built: the variant's hash and
  whether the type is closed (`Ty::is_closed`). A node's children are
  handles too, so a clone copies a pointer and a nested type holds each
  level once.
- Equality compares the handles first, then the digests, and the variants
  only when both agree; hashing writes the digest. A variant that carries
  no data (`Ty::int()`, `Ty::none()`, …) is one node per process.
- A write is explicit: `Ty::update` unshares the node and refreshes its
  facts, and a rewriting walk rebuilds through `Ty::reusing`, which keeps
  the handle it read when the rebuilt variant is equal.
- The checker's `ParamContext` uniques the types it records
  (`ParamContext::intern_ty`, beside the parameter expressions it already
  uniques, as MLIR's context uniques types beside attributes): every
  recorded expression, place, binding, declaration, and conversion type
  becomes the context's node for it when the check hands its facts off
  (`body_carry`'s `interned` over the occurrence records, `uniqued` over
  the declaration types), so lowering, which clones them into registers, shares
  one node per distinct type (`--timings` counts `types.interned` and
  `types.intern_hits`).
- A type whose decoration equality leaves out (`Ty::carries_decorations`:
  a callable's transfer effects, a dictionary or set value's display
  spelling) is never uniqued, since sharing its node would hand one
  occurrence another's decoration.
- Below the check (mono, HIR, MIR lowering, the MIR text reader) types are
  still built without the context, so identity stays structural there:
  equal types may be distinct nodes, and equality falls back to comparing
  them.

### Parameter Expressions

A value argument such as the `n + 1` of `Buf[n + 1]` is a typed, immutable,
canonical node: `mojito_types::param_expr::ParamExpr`, built only by the
canonicalizing constructors of a per-compilation `ParamContext`. Two
expressions the front end can prove equal are the same node, so type equality
over value arguments is decided before any value is supplied. A value
parameter's default, a conditional callable default's condition, and a
dependent type (`DependentType::Parameter`) are the same nodes; there is no
second expression tree.

- The context rides in the checker's `CheckContext`, so every transfer
  round of one compilation shares it. Pure helpers in
  `mojito-types` canonicalize on a detached context; equality is structural
  across contexts.
- The normal form is no stronger than the pinned Mojo's: integer `+`, binary
  `-`, and `*` are a sum of products, a left shift by a constant is a
  multiplication, and unary `-`, `//`, `%`, and `**` are opaque atoms.
- Replacement (type identity) re-folds the polynomial and leaves an opaque
  atom unfolded; evaluation (a default, a dependent index, native
  monomorphization) folds it. The pin draws the same line.
- A reference is owned by its declaration (`ParamId { owner, slot }`), so
  same-spelled parameters of unrelated declarations differ and a `$` clone
  shares its template's. An overloaded module-level `def` is owned by the
  signature-qualified symbol MIR names it by (`tally$ov$…`), so two overloads
  of one name never share a binder. An overloaded struct method is owned the
  same way, by the symbol its template lowers to (`Box.pick$ov$…`):
  `symbol::MethodBinderOwners` computes it once per struct and keys it by
  the struct and the method's declaration (its name and the signature facts
  its symbol spells), never by its body, which a packaged declaration does
  not carry. The lowered symbol spells the method's own binders
  with their bounds (`First.pick$ov$T$Copyable$Int`), a callable bound
  (`F$Callable$def…`), and an existential `Some[Trait]` parameter
  (`Some$u5B$Writer$u5D$$Writer`) exactly as the checker's `TyKind::Param`
  spells a call's selected overload, so the symbol a call names is the one
  MIR declares. The checker and the elaborator
  (`comptime/params.rs`, `elaborated_binder`) both name a method's binders
  through it, and both number a binder by `TypeParam::binder_slot` — its
  position with the erased `Origin`/`OriginSet` parameters and their
  mutability binders skipped — so one binder has one id in both. A trait requirement is owned by
  `Trait.method`, or, overloaded, by the symbol `lowered_method_name` gives
  it (`Picker.pick$ov$T`): `OverloadSets` counts requirements as it counts
  struct methods. A type binder is the same identity: `ParamDecl`
  carries its `id`, `TyKind::Param { binder: ParamRef }` compares by it, and every
  type substitution is a `TySubst` keyed by it; a canonicalized callable
  contract's binders are `$contract` slots, so contracts differing only in
  spelling are one identity and a trait witness may spell its binder as it
  likes. A `where` operand and a callable default's parameter are `ParamRef`s
  as well: `compile_where_clause` leaves an operand unbound
  (`ParamRef::unbound`), `Checker::bind_constraint` binds it at the
  declaration — its own binders, then the open scopes, then the enclosing
  struct's — and a clause is judged under a `ConstraintEnvironment` keyed by
  `ParamId`. An operand no classified declaration binds (an origin's
  mutability parameter, an associated member's own parameter) stays unbound
  and is identified by its spelling. A deferred slot (`CtValue::Deferred`)
  carries the `ParamRef` of the binder whose slot it fills, taken from the
  declaration at each site the checker builds one, so the Tuple closedness
  check (`ClosingBinders`) counts it as bound only by that binder. The
  elaborator's markers for a name that is no parameter are a separate
  variant, `CtValue::Marker(CtMarker)`. A bound-pack query
  (`ParamKind::PackQuery`) names its pack by `ParamRef` as well, so
  `ClosingBinders` is keyed by binder identity alone.
- `param_expr::fold` is the one implementation of compile-time scalar
  operators; the checker, the elaborator, and native monomorphization call it.

`docs/notes/param-expr-attributes.md` is the design record, with the pin
evidence behind each rule.

The implemented forms are:

- `comptime NAME = expr`: evaluates `expr` immediately, records the result in
  the compile-time environment, and keeps a folded declaration whose value is a
  literal.
- `comptime if`: evaluates each condition as a compile-time `Bool` and keeps
  only the selected branch. The dropped branches were already checked by
  source validation, so elimination hides no type error; a dropped branch's
  compile-time evaluation failure (a `comptime k = 1 // 0` there) is not
  an error, as upstream.
- `comptime for`: evaluates the iterable as either `range(...)` or a
  compile-time tuple/list, substitutes the loop variable with a literal, and
  splices a fresh elaborated copy of the loop body for each element. A
  zero-step range has no elements in both this direct evaluator and the
  VM-backed CTFE path; the shared loop predicate simply never enters.
- CTFE calls: a compile-time expression may call a pure top-level `def`,
  including value-parameterized helpers and helpers whose type parameters are
  used only for compile-time facts. The elaborator clones the needed helper call
  graph, folds compile-time-only operations such as a type comparison
  (`T == U`) and `T.size` out of the cloned bodies, and executes the resulting helper through
  HIR, MIR, and the register VM in compile-time mode.
- Materialization: module-level `comptime` constants are inlined as runtime
  literals into later code, so a function can use a constant computed at module
  elaboration time.

Generic `def` templates monomorphize in two classes sharing one worklist,
mangling, and clone generator. A **comptime-class** template (a `comptime
if`/`for` body, or a type pack) must specialize at every reference: resolution
failure is an error, and the template is replaced by its clones (a dead
template is dropped unchecked). A `rebind` keys nothing: the template
carries it into MIR, as a value rebind (`MirInstr::Rebind`) or a place's
`Proj::Rebind` step typed at the target (upstream's `kgen.rebind` on the
reference its `ref` overload returns, so a later projection addresses the
target), and the elaborator judges each one against the type it applies to
per instance once its `comptime if`s are decided and erases it, as upstream's
`processRebindOp` does (`native::mono`'s `discharge_rebinds`,
`mono/rebind.rs`), failing the instance with a `MonoErrorKind::Instantiation`
error on a mismatch.
Every generic `def` is a template, whatever its binders — type parameters,
scalar or aggregate value parameters, type packs, and value packs alike: a
`comptime if`, a `comptime for`, and a `rebind` stay in the template whatever
they read, the elaborator below MIR decides them per instance, and every
call of a generic `def` named once reaches its template, inferred or
explicit, while an overloaded generic name is the checker's to select among,
each declaration served by its own template. Nothing above the waist clones
a `def` or replays a recorded instantiation; the checker enforces each
parameter's declared trait bounds at the call, and the template's own check
holds whether or not anything instantiates it, since what a parametric body
demands of its own parameters is a fact about the template.

A method with compile-time parameters of its own is a generator, as in the
pin: its binders are its struct's followed by its own, its template crosses
MIR, and `native::mono` instantiates it per call. The checker records each
call's solved arguments (`checked::MethodInstantiation`, a literal argument
at its own type, as a `def`'s), MIR carries them on the call
(`MirInstr::MethodCall::instantiated_args`, text schema 1.22, beside the
inferred value parameters passed as named arguments), and the elaborator
binds the method's own binders from them and its struct's from the
receiver. That covers a non-generic struct's method, a generic struct's
instance, a static, a generic constructor (a construction the elaborator
respells to the constructor's instance), a method keyed on its own `DType`
or lane binder (the checker closes the binder in the call's result type),
and a body whose `comptime if` or `comptime for` reads only its own binders,
which the elaborator keeps in the template (`Elab::def_body`) and decides per
call. `native::mono` finds what such a body reaches from its MIR.

A method keyed on a type pack of its own is served the same way: its
template body opens the pack (`Elab::def_body`), and the call's solved pack
binds it below MIR. No method mints an AST clone, per call or per
instance: every `comptime for` a method's body holds is kept in its template
(`keep_template_comptime_for`), its sequence and the bindings its body
declares evaluated per instance below MIR (the iterable's `__iter__()`
thunk for a loop, an R7 request for a binding). A method body that still fails to elaborate
over its own binders or its struct's becomes, on its template, the one
compiler-private call `_mojito_instantiation_failed("…")`
(`instantiation_failure_stub`): the failure is the pin's `function
instantiation failed`, reported only where the program reaches an instance. `native::mono`
reports it per reached instance after branch selection
(`discharge_instantiation_failures`, `mono/failure.rs`) as a
`MonoErrorKind::Instantiation` error. A template-served body fails the
same way where a parameter constant its instance reaches cannot be
computed (`comptime q = [1, 2][i]` past the display): an unrolled
iteration's failure waits in `unreached_failures` until branch selection
has pruned the arms the instance never takes (`reached_failure`,
`mono/failure.rs`). A member of a vector- or struct-keyed
specialization that its template serves keeps its own binders and mints no
clone, as a non-generic struct's method does. In the checker,
`instantiate_method_generics` resolves an inferred pack's variadic element to
the heterogeneous `RuntimePack` so each overflow argument scores and converts
against its own element.

Temporaries borrow for their statement through three anchors. A temporary
receiver of a `ref[self]`-returning method is materialized like a temporary
bound to a `ref` parameter (`materialized_reference_actual`; when the
expression already carries `BorrowRefArguments` or a view result's
`BorrowViewResult`, the owner rides on that adjustment's `materialized` slot,
and `materialized_borrow_owner` is the one reader), and `lower_call_receiver`
stores it in its `$mat_r` slot so the result handle roots at real frame
storage. The `$mat_r` slot establishes the temporary's own loans
(`aggregate_borrows_unmaterialized`, the loan funnel without the slot's
self-loan), so storage a materialized view borrows outlives every borrower of
the slot (`it.peek_next().value()`). The slot is lent with the capability the
borrow that forced the materialization needs, which
`MaterializeBorrowSource` records and `materialized_borrow_mutability` reads:
materializing changes where the storage lives, never what the loan permits,
so a read receiver's two views coexist exactly as they would over a named
local. A subscript view temporary
(`MultiIndex`/`Slice`/`Index` with a `BorrowViewResult`) in any argument
list or operator operand anchors in `$arg_loan_r` like a loan-carrying call
temporary in a plain call — no consumer channel retains it, so the anchor
never duplicates a loan. An operator's operands anchor under the operator
call's own permission (`lower_operator_operand`), so `s.rstrip() == "ab"`
keeps `s` alive through the dunder. The builtin `String(view)` conversion lets its arguments anchor like a
plain call, because its result is a fresh owned string carrying no loans, and
a call temporary at a place-retaining argument slot, which has no place to
retain, anchors through the same `anchor_temporary_argument` decision. A
binding's argument anchors stay live through its store, so assigning a call
straight back to the local a temporary view argument borrows
(`head = f(head[byte=1:4])`) is an ownership conflict, as upstream rejects it.
Upstream keys that rejection on the argument's owned-interior origin rather
than on temporary lifetime, so it also rejects a named view and accepts a
plain-origin one (`s = f(StringSpan(s))`); the roadmap tracks both gaps. A tuple unpack
gives every loan-carrying element (`CheckedTupleUnpackElement::carries_loans`)
the unpacked value's loans, as binding the whole value would. A discarded
reference result (an expression statement, `_ = e`) is
recorded in `discarded_reference_results` and is not a value read. On the
VM, the host string accumulator's `write` and the `$write_formatted`
primitive format their arguments with the caller frame mirrored so a nested
`write_to` can read a pointer into the caller. The empty-subscript
store `p[] = v` types the pointer as a value read and gates on the pointer
origin's mutability alone, so a plain `self` method writes through a
`Pointer[T, Origin[mut=True]]` field as upstream does.

A scalar `range(...)` the checker selects (`Checker::infer_scalar_range`,
once no `Int` overload matches) is spelled in the same check as the
construction of the linked range struct at the inferred dtype
(`_ZeroStartingRange[DType.int32](…)`), a `SemanticAdjustment::SpelledConstruction`
on the call typed as any generic construction, which the checked arena and
HIR lower in the call's place (the call's node operand is the
construction) as they do an initializer list or a t-string.

`Compiler::compile_linked` elaborates once and checks once (since
2026-10-08): no check feeds a second elaboration, and the instances a
program needs are found below the MIR waist by `native::mono`'s worklist
from the entry roots. Nothing is cloned above the check: every generic
`def`, struct, and method is a template the one check infers once, with its
parameters symbolic, and `native::mono` instantiates below MIR.

Methods with their own compile-time parameters (`def isa[T: AnyType](self)`,
a constructor `__init__[T: AnyType, //, F: def() -> T](out self, *,
init_with: F)`) are served by their templates. The checker's method paths
record a `MethodInstantiation` (receiver struct, method, declaration-order
arguments; an infer-only `T` also solves through a callable-bounded
sibling's contract), which the driver reads for the instances the method's
body reaches (`TemplateReach::method_call`), and `native::mono` instantiates
the template's MIR per call. A call whose arguments stay symbolic (inside
a generic body) is instantiated where the body's own instance is.

An *ordinary* generic struct (`struct Optional[T: AnyType]`, every parameter
a plain type parameter) keeps its template in the program — checked once
with `T` as `TyKind::Param` (Mojo's generic pre-check). A method with no
compile-time construct in its body is a generator: its template's MIR serves
every instance, and `native::mono` substitutes into it for both
backends, so nothing is cloned, checked, or derived per instance. What the
elaborator resolves there is what the instance's own check used to: the
witness of an operator or a bound call, a `!=` served by a negated `__eq__`,
the `Bool` conversion of a sized-scalar comparison, an arity-keyed variadic
constructor, and the struct's parameters of a static call, which MIR records
on the call as its spelled receiver type (`MirInstr::Call::receiver`,
`Pair[Self.U].count()`).

A `comptime if` or a `comptime for` over the struct's parameters stays in
the template, as a generic `def`'s does: the elaborator's struct walk opens
the struct's parameters as binders of each method's body (`Elab::def_body`),
as `Self.`-qualified names only, with a collector of the struct's pack
(`*b: *Self.Ts`) a binder as a method's own collector is, so
`native::mono` decides the branch and
unrolls the loop per instance; a `rebind`, a nested `def`, and a lambda stay
too. No method mints a per-instantiation clone, whatever its body holds or
reaches. A type name over the struct's parameters is carried in
`MirInstr::TypeName`, and the elaborator writes the name from the
substituted type. A plain trait-bound
generic `def` keeps its template at every closed call, inferred or explicit
(`Elab::template_serves_def`): it mints a clone only where it carries a
value parameter its template does not serve, and a
clone at a loan-carrying argument is spelled over clone origin binders. MIR
records the compile-time arguments the checker solved on the call
(`MirInstr::Call::instantiated_args`), so the elaborator binds a type
parameter no runtime parameter or result spells (`bytes[Int]()`); an
associated type the signature does not spell (`C.Element` as a loop
variable's type) is solved from `MirStructDeclaration.associated_types` once
`C` is bound, and a parameterized one (`C.IteratorType[origin_of(xs)]`, a
split loop iterator's type) from `parameterized_associated_types`. `native::mono` finds the closed instances a template body
reaches (`Box[List[Self.T]]`) from its MIR, transitively, and selects the
receiver's own overload where a
bound dispatch spells the requirement's parameter otherwise than the
witness does (`Some[Hasher]` against `[H: Hasher]`;
`dispatched_overload_target`). The checker still records every closed
application it reaches as a constructor target or method-call receiver
(`StructInstantiation`), a checked fact nothing above the waist turns into a
request. Copying, moving, destruction, and the by-name dispatches the
backends perform on a runtime struct name (`print`/`String(x)`/`repr(x)`/
`Writer.write` through `write_to`/`write_repr_to`, and `len`/`abs`/`Bool`/
`Int` and the prefix operators through their dunders) select the method by
the instance struct `native::mono` built on concrete MIR.

Under the production compiler, the erased-dispatch machinery
(`__trait_dispatch.*`/`__iterator_dispatch.*` symbols, VM retargeting, and
the `CopyIteratorReference` result adapter) is therefore reachable only
through retained-template residue: function values/indirect calls,
overloaded generic names, generic methods, comptime-class inferred calls,
open instantiations, conflicting unrolled occurrences, and abstract-body
pre-checks. Its verification witnesses live in `mir::verify`:
`verify_iterator_result_adapter`, the `GetIter` undeclared-prepare
tolerance, the subscript abstract-target tolerance, the `MethodCall`
abstract-`__next__` adapter symmetry, `CallIndirect`'s callable-contract
validation, and the direct-`Call` undeclared-callee tolerance — the set the
backend-ready MIR checkpoint re-confirms before freezing the schema.

The important distinction is that the elaborator still owns compile-time AST
rewriting, while function-body execution now goes through the MIR/VM path. The
remaining expression evaluator in `crates/mojito-comptime/src/comptime.rs` is not a second function
runtime; it exists to decide `comptime if`, enumerate `comptime for`, resolve
type-valued compile-time facts, and fold those facts before a CTFE helper is
lowered to MIR.

### Compile-Time Calls

No call is evaluated above the check. A compile-time position that applies
a callable — a local `comptime` binding, a `comptime if` condition, a
`comptime for` bound or sequence, a `comptime(e)` or `materialize[...]()`
operand, a module constant's initializer — is kept for the check
(`applies_callable`, one predicate the elaborator and the checker share),
bound there as a parameter expression (an `Apply` of the callee, or of a
thunk MIR lifts for a general expression, `ComptimeThunks`), and served by
the elaborator below MIR, which runs the instance's reference closure as
concrete MIR on the VM (`Specializer::demand_application`,
`VmBackend::call_concrete`). The elaborator above the check
(`comptime/eval.rs`) folds closed values only and refuses a call
(`not_evaluated_here`); a generic `comptime` alias body that applies one
is the shape this refuses that the pin evaluates (R518).

### Fuel

In this codebase, **fuel** means a compile-time step budget. It is not a runtime
performance mechanism and not user-visible gas. The budget is one fixed
quota per compilation (`mojito_vm::crossing::CTFE_FUEL`, `Specializer.fuel`),
burned by the elaborator below MIR for `comptime for` unrolling and for
every function it runs on the VM (`VmBackend::call_concrete`). If the
budget reaches zero, elaboration fails with a quota error.

The goal is to prevent compile-time execution from hanging the compiler. A bad
`while True` in a compile-time function or an enormous generated loop should
fail deterministically instead of making compilation unbounded. This is
similar in spirit to Zig's compile-time branch quota, though mojito keeps the
mechanism small and fixed for now.

### Checker Interaction

The checker owns compile-time typing: value-parameter positions such as
SIMD widths, the constant folder for closed `Int` expressions
(`eval_ct`), the constraint compiler, and the parameter expressions every
compile-time read denotes. The elaborator runs before it and folds closed
values into literals (`comptime T = Int`, `comptime L = [1, 2]`); every
other compile-time fact crosses the check as a parameter expression.

`ParamDecl::Value` retains its declared checked type and optional compile-time
default, a `ParamExpr`. The shared `CtValue` model carries integers, booleans,
strings, tuples/lists, types, residual parameter expressions, and zero-sized
reflection handles. Both phases fold through `param_expr::fold`.
Only literal-shaped values materialize into runtime AST; type and reflection
handles are consumed and erased during elaboration.

### Exact Numeric Literals

Numeric source literals have dedicated compile-time types and representations.
`IntLiteral` owns a `num_bigint::BigInt`; finite `FloatLiteral` owns a reduced
`num_rational::BigRational` plus a negative-zero bit. Lexer tokens, AST nodes,
`CtValue`, checked constants, MIR constants, and VM CTFE bridges preserve those
values without first passing through an `i64` or `f64`. Literal arithmetic is
therefore exact, subject only to the compiler's explicit exponent/shift resource
quota.

Contextual typing selects the one transition to a runtime scalar. The checker
records `SemanticAdjustment::MaterializeLiteral(target)`, HIR retains that
decision, and MIR emits `MaterializeLiteral { value, target }`; the verifier
requires an exact-literal source and compatible concrete target. Integer targets
use destination-width two's-complement wrapping. Floating targets round once
from the exact rational directly to binary32 or binary64, preserving signed
zero and producing IEEE infinity on overflow. Bindings, stores, calls, returns,
typed tuple/list/set/dictionary elements, and `range` arguments all record their
scalar boundaries rather than relying on VM container coercion. A built-in
scalar constructed from a literal (`UInt(7)`, `Int(7)`, `Float64(2.5)`) is
that literal's materialization, as upstream's builtin-inline
`SIMD.__init__(IntLiteral)` is `pop.int_literal_convert`: `infer_conversion`
records `SemanticAdjustment::LiteralConstruction` on the call and
`MaterializeLiteral` on its argument, and MIR lowers the call as its argument.
The nullary `UInt()` is the literal `0` materialized the same way.

A runtime scalar meets a multi-lane vector of its dtype through upstream's
implicit `SIMD.__init__(Scalar[dtype])`, which the builtin `SIMD` cannot
declare. `types::scalar_splats_into` owns the rule, and the checker's
`implicit_conversion_constructor` selects it as an ordinary implicit
conversion under the reserved target `symbol::SCALAR_SPLAT_CONVERSION`, so
overload filtering, template realization, and argument contracts see one
conversion. The checked arena turns that target into
`SemanticAdjustment::SplatScalar(vector)` (a call argument's
`CheckedCallValueAdjustment::SplatScalar`), which MIR lowers to the
one-element `MakeSimd` an explicit `SIMD[dt, w](x)` emits.

Generic value parameters are also materialization boundaries. Their resolved
`ParamDecl`s cross `CheckedProgram` through stable declaration-owned
`GenericSite`s into MIR declaration metadata, so the VM reifies a value at its
declared type without reclassifying source bounds. This prevents an exact
literal from leaking into an erased runtime slot and keeps hashing, equality,
stores, calls, and returns consistent. The defensive erased-value path compares
finite numeric values in one exact rational domain and hashes that canonical
form, including treating positive and negative zero as numerically equal.

A parameter default is declaration metadata too (`CheckedConst`): a literal,
an `@implicit` conversion of one (`Construct`), or, for any other default,
`Evaluate`, a zero-parameter function MIR lowers from the default expression
(`$default$<owner>$<parameter>`). Every default is the call's to evaluate,
as upstream's call binding writes the callee's default attribute into the
call: the lowering of each checked call records the callee it selected and
the operands preceding its regular slots (`MirFunction::lowered_calls`,
a `LoweredCall` per call), and once every declaration is lowered and
linked, before ownership analysis, `mir::fill_omitted_arguments` binds each
recorded call against its callee's declaration and writes every omitted
argument into the call: a literal's constant materialized at the parameter
type, a conversion's constructor run on it, or a call of the default
function under the call's own `instantiated_args`, the receiver type's
struct arguments, and the enclosing binders in scope, each passed in the
omitted slot through the read-temporary or forwarding slot a written
argument would get, so drop elaboration destroys a borrowed default after
the call. A direct call, a method call, a static-method call, a
construction, a subscript (an `Index` carrying one becomes a `MultiIndex`),
a nested `def`'s direct call (its captured places lead) or its closure's
`call.indirect` (its captures lead the lifted body's parameters), a call
through a trait bound to a trait default's body (the requirement's dispatch
symbol names the member body, whose `self` leads), and a function value's
`call.indirect` (the function its register statically holds,
`mir::callable_targets`, a variable every definition of which holds the
same function included) are all filled; a default whose binders the call
does not bind is an invariant error. The default of an argument the callee
never uses is filled too, and the elaborator drops the operand as it drops a
written one; the view of the caller it instantiates, with every call so
rewritten (`mir::with_removed_argument_forwards_erased`), erases the
default's `$default$` call and the read temporary bound to it, its
destruction included, so the call evaluates nothing for the argument, as on
the pin. The callee evaluates no default: the
elaborator enqueues no default function from a declaration, and a slot a
call shape still leaves out is an explicit `Unsupported` error on both
backends. A default reading a binder in scope (`Self.n`, an enclosing `n`,
`T`) declares the binders in scope as its function's parameters, which the
call binds; the callee's instances are keyed without a binder only its
default reads, as upstream's callee names no default. A nested `def`'s
default reading an enclosing value parameter reads it as a parameter, as
its body does. The checker rejects a default naming runtime storage, as the
pin does.

Current Mojo reflection enters through the zero-sized `reflect[T]` compile-time
handle. Mojito implements `is_struct`, `field_count`, `field_names`,
`field_types`, and `field_index`. The type-valued aliases `.field[name]` and
`.field_at[index]` return another reflection handle, so selection composes across
nested structs and the terminal handle's `.T` supplies the concrete dependent
type. A reflected type list can likewise be indexed in type position, so a
specialization may use `types[i]` as a concrete dependent type. The removed
`field_type` alias is diagnosed during elaboration rather than leaking into
ordinary member or call checking.

## Stage 3: Semantic Checking

Entry point:

```rust
checker::check(program: &[Stmt]) -> Result<(), TypeError>
checker::check_program(program: &[Stmt]) -> Result<CheckedProgram, TypeError>
checker::check_program_in(program, context) -> Result<CheckedProgram, TypeError>
```

`check` is the compatibility validation wrapper, and `check_program` the
whole-handoff entry phase tests and direct clients use, in a default
`CheckContext`. The compiler driver calls `check_program_in` with the
compilation's context (its `ParamContext`, the body-fact carry switch, and
the requirement defaults bound at calls through a bound), which runs every
rejecting check — the transfer-effect fixpoint, reference-result reads,
context-manager splicing, explicit destruction — and then assembles the
checked arena. The check hands the arena builder a `DiscoveryResult`,
`CheckedProgram::new`'s internal input until the check emits its nodes
itself: the check's facts as one `NodeFacts` record per occurrence, as an
op holds its attributes, beside its declaration-site tables (annotation
types, generic parameters, declaration effects) and its struct
instantiations, with `DiscoveryResult::finalize` for the arena. A
`DiscoveryResult` is not executable, never reaches HIR or MIR, and no
client reads it. `CheckedProgram` holds no syntax: it is the checked
nodes, the site facts, and the declaration-level tables. Its declarations
are made by a walk over the statement nodes (`CheckedTables::declare`),
each declaring node naming its own, and a `def`'s explicit captures are
its op's (`CheckedDef::captures`), as upstream's closure op holds its
captured values.

The passes of one check hand each other a `PassCarry`: the pass's fact
stores, one record per body site, and its binding-identity watermark. The
checker writes each fact straight into its occurrence's one `NodeFacts`
record (`Checker::record_at`, `checker/node_facts.rs`), as upstream sets
an attribute on the occurrence's op, so the handoff is that store moved
out, less any record a cleared fact left empty
(`body_carry.rs`'s `HandoffStores::into_discovery`). An overload
candidate — of a free call or of an overloaded function value — is
checked inside `Checker::probe` (`checker/probe.rs`): every store
journals what a write displaces while a probe is open, and closing it
restores those values and truncates the log, so a losing candidate leaves
neither a fact nor a logged key, and only the selected candidate's re-run
writes for good. A probe costs what the candidate writes, and probes
nest. Every
fact store the checker writes during a body is a logged store
(`mojito-checked`'s `fact_store.rs`: a map, set, or vector plus the log of
keys written), so a site — a module-level `def` or a struct method —
records the log ranges it spans and the effect entries it read exactly as
read (`checker/body_carry.rs`). The next transfer pass over the same tree
starts from the previous pass's committed effect maps and, at a site whose
reads still equal the committed entries, copies the logged entries instead
of inferring. Binding identities never
collide: a pass starts its counter at the previous watermark, and a body
inferred again allocates inside the range it had (`scopes.rs:reserve_owners`)
so an unchanged prefix keeps its identities. The copy is exact by
construction — nothing outside the body window is skipped — and
`tests/compiler_test.rs` pins that a compilation with carry-over lowers the
same MIR as one without (`MOJITO_BODY_FACT_REUSE=0`), up to identity
numbering.

The checked handoff owns the elaborated AST for diagnostics and
an explicit semantic arena. Every checked expression has a `CheckedNodeId`, its
own operation (`CheckedOp`: the node's `ExprKind` with each child expression
replaced by the child's node, so its operands are its child edges and a type
operand is the attribute the check resolved), its occurrence, resolved runtime type, value/place/type category, stable owner identity for
binding uses, extensible effect facts, and a list of semantic adjustments. Checked
declarations have independent `CheckedDeclId` identities. A statement is a
checked node too (`CheckedStmt`, with a `CheckedStmtId`): its own operation
(`CheckedStmtOp`: its `StmtKind` with each expression operand a node and each
body a list of statement nodes, so `if`, `while`, `for`, `try`, `with`, and
the compile-time forms stay structured as upstream's `hlcf` keeps them), its
occurrence, the binding it introduces with that binding's type, and its
declaration. A declaration's op holds what the check resolved for it, never
its signature as written, as upstream's function op holds its symbol and its
resolved function type: a `def`'s and a method's op hold the symbol it lowers
to (a nested `def`'s own name, which its enclosing lowering qualifies), a
method's its registered name (`__copyinit__` for a copy constructor), and a
`CheckedSignature` read from the check's declaration sites (its compile-time
`ParamDecl`s, each declared parameter's `Ty`, its result `Ty`, and its
`DeclarationEffect`); a default wrapper holds one too, and a struct's op holds
each field's checked type (`CheckedField`). A function's root statements are its op's body; the tables keep
the module's and each default wrapper's. A lambda's op names its `def`'s
statement node. HIR walks the statement and expression nodes, and every CFG
builder is handed its body's statement ids: a unit's from its declaration's
op (a `def`'s body, a method's, a trait member's `def` node, which its
trait's op names (`CheckedTraitMethod::default`), a wrapper's `CheckedTables::wrapper_roots`), the top level's from
`CheckedTables::roots`, a nested `def`'s or lambda's from its `CheckedDef`,
a `try` region's from the op, and an expression thunk's its prologue
bindings' nodes and its returned value's node (`Cfg::build_checked_thunk`).
No checked statement is found by its source location. MIR lowers an opaque
statement and a `try` by matching its `CheckedStmtOp` alone, each expression
operand the handle of the matching `HirStmt::expressions` entry, in the op's
operand order; it reads each declaration's symbol, signature, and field types
from its op, looking up no declaration-site row, and finds nested
`def`s and lambdas by walking statement ids. Source spans index
diagnostics and associate a checker-approved compile-time value argument with
the register produced from that same source occurrence; they are not used to
re-resolve a type, overload, effect, or origin decision.

Call targets, implicit conversions, moves, explicit-destruction decisions, and
reference-handle preservation are stored canonically as `SemanticAdjustment`
values on checked nodes. `HirExpr` names its node and recursively retains its
operands' trees in `CheckedOp::operands` order, with the place HIR computed for
each; it holds no syntax and no copy of a node attribute. MIR rebuilds no syntax
for a body expression or statement; a local name is spelled as the slot
`Cfg::slots` resolved it to (`name_spelling`) only where MIR names a slot,
which is its checked binding's slot (`Cfg::bindings`), the one MIR reads.
MIR reads every checked fact off a node handle,
`Node` (`mir/node.rs`): HIR's tree for the expression with its checked node,
operand `i` being the `i`th of the op's `CheckedOp::operands`, as upstream
lowering holds an operation and reads its operands and attributes off it. The
node's type, owner, call contract, adjustments, and operand shapes are `Node`
methods, and the loans and capture accesses that also need the function being
lowered are `Flatten` methods taking one (`mir/facts.rs`). `Flatten::expr`, its adjustments, `reference_handle`, the place
lowerers, the compile-time argument slots, and the call-site argument,
receiver, anchor, and invalidation lowering take a handle and read operands
off it, and so does every expression family `expr_unconverted` dispatches
to, the call family's callee (`Node::callee`), receiver, arguments,
keywords, and compile-time arguments included, and so does statement
lowering: a binding's value, a store's place and value, an augmented
assignment's place and operand, a subscript store's slice bounds and
multi-subscript arguments (the ids the op names), and a parameter default's
binder scan (`EnclosingBinders::named_by`, a `CheckedTables::walk_expression`
over the default's node that reads only resolved attributes: a bracket
argument's `TyArg`, a type operand's `Ty`, and a lambda's checked function
type, never a nested signature's annotation syntax). `Flatten` borrows its CFG's
tables and HIR for its own lifetime, so a handle outlives the `&mut self`
calls it is passed to; a local display binding holds its value's handle,
which every crossing lowers in place, and a `try` region's bindings go out
of scope with its CFG, as the check scopes them (`Flatten::displays`).
Literals and name
reads lower by matching the node's `CheckedOp`, reading the node's own
attributes (`Flatten::literal_or_name`); a name's slot is its binding's
(`binding_var`), and HIR's `Cfg::slots` entry for the name is that same slot, and the name predicates of the
other families match the op too. Calls, callable-value invocations, method
calls, and callable type applications dispatch the same way
(`Flatten::call_family`): the op decides, the entry points read the callee,
the method, and every adjustment from the node, and a call's loans
transferred to its arguments are the node's own `call_transfers`. Member
reads, subscripts, slices, multi-subscripts, and `Variant` projections
dispatch the same way (`Flatten::access_family`), reading the field name,
the subscript arguments' kinds and keywords, and every adjustment from the
node; the place builders, the subscript stores, and the projection
predicates decide by the op too, and each operand (a slice bound, a
subscript argument) is the handle of the id the op names. List, brace, and tuple displays and comprehensions dispatch
the same way (`Flatten::display_family`): the constructor and insertion
method come from the node's construction adjustment, each comprehension
clause's kind, binder, and iterable come from the op, and a display without
a checked construction is an explicit `Unsupported` boundary, never a
collection guessed from its delimiters. Operators, comparison chains,
ternaries, transfers, spreads, and lambdas dispatch the same way
(`Flatten::operator_family`): the operator comes from the op, and each
operand is the handle of the id the op names, so no
expression family is chosen from syntax and an expression without its
checked node is one explicit `Unsupported` boundary. A static
call's receiver is a type because its node is a name the check bound to
nothing and typed as no value, never because no local shares its spelling.
A subscript accessor's compile-time value argument names its operand by
bracket position (`CheckedCallParameterArgument::operand`), as upstream's
accessor call carries its parameters positionally, and reuses the register
already evaluated at that position. Register spans serve diagnostics only.
SIMD dtype and width selection is likewise a checked adjustment rather than
MIR-side syntax evaluation. Span-indexed call/conversion maps remain only as
public compatibility queries and are not part of lowering.

Specialization values include `CtValue::Dtype`, nominal `CtValue::Struct`
instances, `CtValue::Trait` (a trait bound to a trait-valued parameter,
which has no run-time form, and which a type binder bounded by that
parameter, `T: Tr`, is checked against at each use), and `CtValue::Pointer { memory, offset }`
leaves. A live pointer
freezes its allocation recursively; an absent slot remains uninitialized,
`None` memory denotes a dangling pointer, and an empty allocation remains
separate from a dangling pointer. Dead allocations and cycles reject
explicitly. Each pointer freezes independently, so allocation aliasing is
not preserved (R522). A `CtValue::StoreToMem` is a frozen reference into
compile-time memory, upstream's `#lit.store_to_mem`: the value it reads,
which thaw stores in fresh compile-time memory. It has no run-time form.

A `Const::Value` carries these values across the verified MIR waist. The
VM's typed `thaw` and Pliron's `store_parameter_value` allocate fresh storage
at each materialization, write initialized slots at the checked element
type, and reconstruct nested strings. CTFE call arguments use the same VM
thaw path before execution. Nominal collections retain their memory and
layout rather than rerunning their constructors. The checker owns the
`ImplicitlyCopyable` crossing rule; an explicit `materialize[...]()` can
cross a non-implicitly-copyable value. Requested binding reads that need a
place record a hidden materialized borrow owner, including reads within
compile-time query thunks. A string read at the nominal `String`, and an
aggregate holding one, is the same one operation: `native::mono` emits the
`Const::Value` typed by the read's register and never calls the value's
initializer, so an `__init__` that transforms its arguments runs once, at
compile time, as the pin's does. A display constant (`comptime L =
[M(1), M(2)]`) is the `Array` its lifted application's CTFE builds and
freezes over pointer memory: `ParamContext::answer_queries` keeps that
frozen value rather than flattening it to its elements, and each read
materializes it with no element initializer run. The one residual
construction is a reflected field-name list read at an `Array`, built at
each read by the list-literal initializer over its names
(`array_display_construction`, R900). A local binding of an
application is that application at each read, as a module constant is: it
has no slot, and every read materializes a fresh value.

`SemanticAdjustment::SelectedCall` is the canonical method-like boundary for
ordinary method calls and method-dispatched nominal subscripts. It records the exact lowered
target, executable result type, and typed raising effect; declared
receiver/argument place requirements independently from origin-solved effective
access; source-to-parameter binding,
including defaults; capture origins; reference-result origin; and source-ordered
compile-time parameter arguments and declarations. `SliceDescriptors` is an
orthogonal adjustment, so descriptor selection and call semantics coexist
instead of overwriting one another. Because a descriptor is compiler-synthesized
rather than a checked source child, overload selection permits only exact or
descriptor-family coercion for it; arbitrary user `@implicit` construction is
rejected before HIR/MIR. Ordinary index operands remain recursively checked
source expressions and retain their selected executable conversions.

The arena deliberately does not encode constructs as a closed expression opcode
ABI. Non-exhaustive declaration/category/adjustment families and independent child
edges allow future pattern binders, class declarations and dispatch, coroutine
suspension, generators, or continuation resume edges without pretending those are
ordinary calls or exceptions.

The older compatibility query remains available to focused tests:

```rust
checker::resolve_overload_targets(program: &[Stmt]) -> Result<HashMap<Span, String>, TypeError>
```

The checker consumes the parsed AST and rejects programs that the later compiler
does not want to reason about.

It is responsible for:

- names and local scopes
- builtin scalar types
- struct declarations and field layouts
- function and method signatures
- overload sets and selected overload targets
- trait declarations and a limited trait-conformance model
- trait receiver conventions and associated compile-time facts
- type parameters and value parameters
- call argument matching
- default, keyword, and variadic arguments where supported
- `var`, `mut`, `ref`, and `deinit` conventions
- compile-time integer constants used as value parameters
- nominal collection, private runtime-pack, Variant, string, and SIMD type rules
- typed and parametric raising effects, inferred handler types, and the `Never`
  bottom type
- borrow checking for call arguments
- rejecting parse-only syntax whose semantics are deferred

The checker is deliberately conservative. If a construct is parsed but not
semantically implemented, this is where it should normally become
`TypeError::Unsupported`.

An implicit assignment introduced inside a branch or loop is allocated in the
enclosing function's stable binding scope, not the transient block scope. The
checker separately carries a set of maybe-uninitialized owner identities across
CFG-shaped source joins: the name remains resolvable after the block, but a read
is rejected unless every reachable predecessor initialized it. Loop joins retain
the zero-iteration path.

Explicit declarations instead keep one runtime slot per stable checked binding,
including same-spelled declarations in sibling scopes produced by compile-time
unrolling: an unrolled `comptime for` copy or a selected `comptime if` arm
that declares a binding is an elaborator-emitted `StmtKind::Scope`, a straight-line block every phase treats as
a lexical scope, and a template derivation counts each copy of a declaration as a
local of its own (`renumber_locals`). HIR may suffix the internal slot name, but downstream place lookup is
by owner identity; independently inferred sibling types therefore never merge
in `MirFunction::var_tys`. A substituted local `ref` alias has no runtime handle
payload, yet its analytical slot retains the checked `TyKind::Ref` capability used
by every `MirPlace::through` access. Opaque structured statements may retain a
source spelling after HIR has assigned a shadowed declaration a suffixed slot,
so MIR resolves identifier writes through the checked `OwnerId` exactly as it
does reads; the two halves of augmented assignment cannot select different
same-spelled bindings.

Public collection types, including heterogeneous `Tuple[*Ts]`, cross the checked
boundary as `TyKind::Struct` with their concrete type arguments. `TyKind::ComptimeList`
exists only while materializing `CtValue::List`; `TyKind::Tuple(Vec<Ty>)` is the
compiler-private heterogeneous pack carrier and is never the type of a public
tuple expression. `TyKind::Variant(Vec<Ty>)` is the compiler-private tagged-union
storage of the bundled `__VariantStorage[*Ts]` field behind `std.utils.Variant`
(gated on `bundled_stdlib_declaration`, never the type of a public expression);
it retains alternative order at the checked boundary. Storage construction and
the parameterized `isa[T]`, projection, and `set[T]` operations record the
selected alternative as a `SemanticAdjustment`; MIR therefore receives a
numeric tag and never guesses one from source spelling. The public `Variant`
API, its conditional conformances, and its `__eq__`/`__hash__`/`write_to`/
`write_repr_to` bodies are Mojo in `stdlib/std/utils/variant.mojo`, specialized
per instantiation and per type-keyed call like any variadic struct. `Value::Variant` repeats the checked
alternative list beside its active tag as a defensive runtime consistency check.

Trait refinement is flattened during checking: inherited method and associated
compile-time requirements become part of the refined contract, and a refined
bound satisfies its ancestors. Trait defaults follow upstream's
`StructEmitter`: each default is its trait's member `Trait.method` (the
requirement's binder owner, signature-qualified when overloaded), one
function over the trait's `Self` whose first compile-time parameter `_Self`
is bound by the trait and whose first argument is the receiver
(`checker::trait_member_declaration`). It stays in its trait statement:
the elaborator elaborates its body in place, the check checks it once after
the source-order walk, so its body calls any module function wherever
declared (as at the pin), and `CheckedProgram::trait_members` carries it to
MIR, whose trait arm lowers it, its calls on `self` dispatching through the
bound. A package places it in its trait's declaration record. A trait's
check records each of its defaults in its `TraitInfo` as a
`trait_defaults::DefaultShape` naming its requirement row by overload
index, which a package exports in the trait's declaration interface.
Each conformer that does not spell a
defaulted overload gets a wrapper the check builds, as
`synthesizeDefaultTraitMethodWrapper` builds one from the trait function's
resolved signature: the check plans every wrapper before its overload
prescan (`plan_default_wrappers`), so a spelled method sharing its name is
qualified as an overload, registers its signature after the struct's
spelled ones from the default's, with `Self` the conformer and its binders
owned by the wrapper (`reown_generic_signature`), and, after every trait
member, checks its body, one statement calling the trait's member with
`Self` bound first and every argument forwarded
(`populateDefaultedTraitFunction`). The wrapper is never syntax of
its struct: its sites follow the spelled methods' indexes, and
`CheckedProgram::default_wrappers` carries it to MIR, which lowers it
through the same `lower_struct_member` as a spelled method. Its
declaration is `nodebug` (`CheckedDefaultWrapper::INLINE`), whatever its
struct, as `synthesizeDefaultTraitMethodWrapper` sets it, so
`inline-parametric` replaces a statically resolved call with the wrapper's
forwarding call, and only a dispatched call reaches a wrapper instance, as
on the pin. Its parameters'
defaults are its member's, held once by the member's lowered declaration,
as upstream's wrapper takes them from `getTraitFunctionSignature`: MIR's
`adopt_member_defaults` copies them onto the wrapper's declaration once the
package is attached, an evaluated default applied to the wrapper's
instantiation of the member (`CheckedConst::Evaluate::arguments`, which its
forwarding call records), so the elaborator binds the member's default
function from the wrapper's binders. Defaults are
inherited per overload; an explicit struct method suppresses the default of
its own parameter shape (upstream suppresses only by satisfaction, R570), and
two different defaults reaching one conformer are rejected as at the pin. So
an importer checks no library default and reads no library trait's syntax: a
`Writer` conformer's `write` calls the package's `Writer.write`. The member
is still checked through a `def` the check builds, with `Self` respelled
`_Self` and its receiver an ordinary first argument, and a wrapper reaches it
through a module binding under its symbol (R594, R595). MIR and
the VM retain static dispatch and need no trait-object representation.

Associated compile-time members may be monomorphic or parameterized.
`TraitComptime` and `StructComptime` retain a name and requirement/value plus an
optional member-local parameter list (type, value, and origin parameters, with
the `//` infer-only boundary), and `Type::Assoc`/`TyKind::Assoc` carry application
arguments (`TyArg::Ty`/`Val`/`Origin`, the origin erased from the runtime ABI like
a pointer origin). A type-parameterized member instantiated by a conforming struct
resolves concretely by substituting those arguments into the member's lowered
template. The bundled owned iterator protocol uses the monomorphic
`IteratorOwnedType` member. Current Mojo's borrowed
`IteratorType[iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]]`
and the dependent application `Self.IteratorType[origin_of(self)]` parse, check,
and arity-validate.

A struct's own origin arguments are part of its checked identity: `TyKind::Struct`
carries its declared type and value arguments first and then an **origin tail**,
one `TyArg::Origin` per explicit `Origin`/`OriginSet` source parameter in source
order (`P[origin_of(xs)]` and `P[origin_of(ys)]` are different types, as
upstream). `Origin::Unbound` marks a slot the use site infers (a bare or `_`
parameter or initialized-local annotation, an alias body); `Origin::Param` in a
tail is the struct's own binder, rigid inside its methods. `Origin::coerces_to`
is the tail rule: unbound accepts anything, everything else demands identity or
containment. The tail erases below the checker exactly like a pointer origin —
mangling emits nothing for it, MIR verification compares struct prefixes only,
monomorphization treats origin-differing instances as one (the native instance
key erases its arguments' origin tails, since the instance symbol spells none —
keying on them would mint a second instance under the first one's symbol), and
the checker's
instantiation records reset it to unbound so a place origin (a per-check owner
id) never makes an instantiation look new across passes. An instance's
bindings are its key's origin-erased form (`Bindings::erase_origins`, sharing
the key's one erasure `origin_erased`), as upstream binds a type parameter to a
type whose origins read `origin={ }`, so no caller's binders or places leak
into the instance whichever caller demands it first. The instance's own origin
binders and its body's own places stay symbolic, as upstream keeps an
instance's origin parameters unspecialized (`$0`, `$1`), so concrete MIR
admits `Origin::Param`.

A type's node caches whether it is closed (`Ty::is_closed`: no parameter,
projection, literal or inference type, callable, pointer, reference,
expression, deferred value, or bound origin at any depth), so `is_symbolic`,
`mentions_open`, `substitute`, the literal, origin, and callable-environment
canonicalizers, and every `TyRewrite` that does not opt in through
`visits_closed` pass a closed type whole (see Types).

An `origin_of(self)` argument on a trait method's *abstract* signature has no
bound `self` place, so it lowers to the symbolic `Origin::SelfParam` — the
`Origin`-level analogue of the signature contract's `SigOrigin::Self_` — which
carries the receiver origin through the associated-type application and, like
every non-`Place` origin, erases from the runtime ABI (collapsing to the single
mangling marker). A conforming struct then resolves the origin-parameterized
member concretely, so a requirement returning `Self.IteratorType[origin_of(self)]`
is satisfiable and conformance succeeds. The borrowed `Iterable` proof protocol
uses this origin-parameterized member, and the bundled List/Set iterator
carries its origin in its origin tail (with an infer-only `Bool` binding its
`mut=`, which erases), iterates itself through upstream's
`IteratorType[...] = Self` alias, borrows its source through a `ref` field, and
yields element references whose mutability the checker resolves from the
source at each loop site. Concrete List/Set/Dict borrowed iteration
keeps its checker-attached interior `element` loan; the mapping iterators
remain snapshot/copy bridges until mapping invalidation lands.

Trait method requirements retain `raises` and an optional concrete error type.
A nonraising implementation may satisfy a raising requirement; a raising
implementation must not widen the requirement's error family. Bounded method
selection substitutes that contract, records it on the checked call expression,
and passes it through HIR to the call instruction just like direct dispatch.

Mojo defines one additional directional refinement for methods named
`__next__`: a concrete `ref[o] T` return may satisfy an abstract value return
`T` when the referent is identical and `T: Copyable`. It does not apply in the
reverse direction or to linear referents. Trait registration retains the
concrete reference ABI, while the abstract call contract carries
`CheckedResultAdapter::CopyIteratorReference`. The adapter survives HIR and MIR;
after runtime dispatch selects the concrete declaration, the VM tests that
declaration's return ABI and, only for a reference ABI, performs the semantic
read and `__copyinit__`. This ABI test is essential because a value-returning
method may legitimately return a reference-valued element. MIR verification
requires the adapter on abstract value-result `__next__` calls and forbids it on
concrete calls. The VM temporarily restores both the executing caller and the
identified just-completed iterator frame while user copy code runs; reference
handles nested inside a `Copyable` result therefore resolve against real storage
and any permitted write-through is preserved. The native backend applies the
same test at both sites: a `for` loop's own advance copies the element out in
`lower_try_next_reference`, and a direct `iterator.__next__()` through the
contract copies it in `copy_reference_result` (`lower/methods.rs`), keyed on
the resolved callee's compiled `ResultForm`.

Opaque trait-bounded collection indexing uses the requirement signature for its
index and result types, then executes through concrete dunder dispatch after
erasure. The standard `Indexer` contract is modeled separately. When overload
selection needs an `Int` index and no direct subscript overload accepts the
source index type, the checker records the exact `__mlir_index__() -> Int`
normalization. Lowering evaluates the source expression once, emits that selected
call explicitly, and gives the later index operation its `Int` result; a backend
does not rediscover the conversion from the runtime value. The VM uses `Int` for
that result because it has no distinct MLIR index representation.

Scalar operators, comparisons, conversions, and rounding are typed through
checked operation traits, not ad-hoc numeric rules. Each binary/prefix operator
names a trait (`Addable`/`Subtractable`/…/`Comparable`/`Equatable`/`Negatable`,
the bitwise and shift ones integer-only) whose dunder the checker resolves; the
same traits serve as generic bounds, so `def f[T: Addable]` type-checks and a
struct declaring an operation trait must define its dunder. Conversions
(`Int`/`Float64`/`Bool` via `__int__`/`__float__`/`__bool__`) and `abs`/`round`
(`__abs__`/`__round__`) route the same way for concrete structs, matching the
paths opaque parameters already used. Builtin scalars keep primitive execution
(`apply_infix`/`apply_prefix`) behind the protocol; a struct operand dispatches
through its dunder (`apply_binop`/`apply_prefix`, and the `abs`/`round`/`Int`
builtins). The checker records every struct operator's selected dunder as the
operator's `BinOp.resolved` — a lone one as `{struct}.{dunder}`, an overloaded
one by its overload key — so the operator is that method call: ownership treats
a resolved `BinOp` as a call, and its operands' owners die at the operator, as
after `a.__add__(b)`. A comparison over a type parameter bounded by
`Equatable` or `Comparable` names the bound's requirement
(`__trait_dispatch.__lt__$ov$Self`), so it too is a call; the elaborator
retargets it at a struct instance and clears it at a builtin one. Any other
operator over a type parameter names no dunder; the elaborator retargets it at
each instance. A chained comparison has no occurrence per link, so its own
record holds one `CheckedCompareLink` per link (the selected dunder and
whether it is Equatable's default `!=`), and MIR emits each link as that
dunder's `BinOp` through the same emitter as a lone operator
(`Flatten::emit_comparison`). Right after the first link, the first middle
operand becomes an owned temporary in a hidden `$chain_middle` slot (a
place copied, a temporary or `^` moved), which drop elaboration destroys
after the second link or on the first link's false edge; a later middle
place is read where it lies by both of its links.

Augmented assignment (`place OP= rhs`) on a user-defined value dispatches to the
dedicated in-place dunder rather than the binary operator: the checker selects
`__iadd__`/`__isub__`/… as a full `mut self` `CheckedCallContract` and records it
as an `AugmentedInPlace` adjustment on the place, so lowering emits an ordinary
receiver-committing `MethodCall` (the mutation writes back through the receiver's
slot, alias, or reference handle) instead of a `BinOp` read-modify-write. A
module `def` call place (`text(s) += "d"`) has no selected-call contract, so the
record also keeps the call's `ReferenceResult` it shares a span with, and MIR
reaches the handle through it. There is no fall-through to `__add__`; a missing in-place dunder is a checker error.
A value of a bare type parameter whose bound requires the dunder dispatches
through the bound the same way, as the call `x.__iadd__(y)` would; a parameter
whose bounds do not require it keeps the operator path. The call's result
register takes the contract's `None` result, since a dispatched target names no
declaration. Native scalar targets keep the primitive `BinOp` path. A user-struct
nominal-subscript element dispatches the same way, recorded on the
`CheckedAugmentedSubscript`: lowering materializes the element into a mutable
temporary and sends the mutated result through `__setitem__` (value getter) or
reads it through the reference handle (mutable-reference getter), applies the
in-place dunder to that temporary, and commits the result.

Printing and `String()` require `Writable`. A custom `write_to` or
`write_repr_to` receives `Some[Writer]`; `Writer` is the bundled trait
(`std/format/__init__.mojo`), whose default `write` writes each heterogeneous
Writable argument through its `write_to`, and a builtin's `write_to` feeds its
host-formatted UTF-8 text to `write_string` (`$write_formatted`). The VM's
buffer writer uses `String` as the `StringSlice` representation. When a Writable
method is absent, field reflection produces the default display or repr form.
`String.format` supports automatic/manual fields, repr selection, escaped braces,
and accepts format-spec syntax; the scalar width/alignment mini-language remains
representation-level work rather than a return to deprecated `__str__` hooks.

Hashable values contribute to a caller-provided hasher through
`__hash__(self, mut hasher: Some[Hasher])`; a conformer that omits it receives
a synthesized field-by-field default at elaboration (next to the `copy`
synthesis). `hash`, `Dict`'s bucketing, and the bundled `AHasher`/`Fnv1a` are
ordinary stdlib code in `std.hash`. The compiler contributes only the leaf:
a scalar or literal receiver's `__hash__(hasher)` is an intrinsic that
normalizes the value (`-0.0` folded to `0.0`, a `DType` to its `UInt8` code,
a `Bool` to `Scalar[DType.bool]`; a `StringLiteral` materializes to `String`
and hashes as that struct) and calls the hasher's `_update_with_simd`, a
generator over the leaf's dtype and width that `native::mono` instantiates
per leaf type and both backends select by the lane shape of its value
parameter (an erased run calls the template with the binders read off the
value) — and a constructible type parameter (`H()`
under a `Hasher`/`Defaultable` bound) is a `MirInstr::ConstructTypeParam`
naming its binder, which `native::mono` writes as the bound type's default
construction. A type bracket argument has no run-time form: as Mojo's call
carries a type parameter as an attribute, it rides the call's instantiated
arguments, and a forwarded binder (`hash[Self.H](key)` in
`DictEntry.__init__`) is a `TyKind::Param` there, which substitution closes under
the caller's bindings (`Bindings.types`, keyed by `ParamRef`), so an explicit
`Dict[K, V, SumHasher]` hashes under `SumHasher` on insertion as well as
lookup. A value argument built from the caller's own value binders
(`successor[n, 1 + n]()`, `below[Self.n, k]()`) keeps its runtime register
for the VM and records the expression over those binders beside it
(`MirParamArg::expr`, built by `Flatten::forwarded_value` through
`ParamContext`), which the monomorphizer evaluates under the caller
instance's bindings (`bind_explicit_value_arguments`, `eval_ct`). A closed
bracket argument of a generic call (`tup[(1, Tag(8, "x"))]()`, `s[1 + 2]()`,
`f["x"]()`) or of a construction (`Counter[4](1)`) has no run-time form: as
Mojo's call carries a parameter attribute, the call carries the value in its
instantiated arguments, the checker names the argument by its position
among the call's bracket arguments, as Mojo's call op carries its parameter
attributes positionally (`SemanticAdjustment::FoldedParameterArguments`, over
`CtValue::is_parameter_attribute`), and MIR lowers no code for it and leaves
its slot's register absent. A construction's instantiated arguments are the
struct's followed by the selected constructor's own, the layout its
declaration restates them in (`Counter[4](1)` calls `Counter[4].__init__`).
A value parameter the call leaves to inference (`size(Counter[4](1))` binds
`n = 4`) has no slot at all: its value rides the same instantiated
arguments. The verifier accepts an absent register, or a missing inferred
parameter, only while the call carries the value (`verify_param_arguments`),
and the monomorphizer binds it from the instantiated arguments
(`bind_instantiated_arguments`). Only an argument over the caller's binders
or a callable keeps a register; a type argument has none
(`verify_param_arguments` rejects one), and concrete MIR keeps no slot
(`drop_parameter_slots`, `verify_concrete`).

Examples of syntax that may parse before it is fully implemented include richer
trait features and advanced expression/declaration forms that the VM does not
yet execute.

### Generic Bodies Are Checked Once

A generic body is inferred once, with its parameters symbolic, and nothing
above the MIR waist derives, certifies, or clones an instance of it: the
instances a program needs are built below the waist by `native::mono` from
the template's MIR, and every obligation an instance owes (a `rebind`
equality, an implicit copy, `Movable` at a transfer, the availability of a
closed method call) is the elaborator's to judge per instance. The living
contract is [`docs/notes/generator-contract.md`](notes/generator-contract.md).

- **Selection is bound once.** A call inside a generic body is bound while
  the body is checked, and every instance inherits that overload and never
  ranks the set again, as the pinned Mojo binds a call while it checks the
  generic body.

### Overload Resolution

Top-level functions, methods, trait requirements, and constructors may form
overload sets. The checker represents a repeated top-level `def` name as
`TyKind::Overload(Vec<Ty>)`; struct and trait methods use a per-name list of
`MethodSig`s.

Duplicate-equivalent signatures are rejected. Distinct arities are naturally
different signatures, and same-arity signatures are allowed when their parameter
types differ.

At a call site the checker:

1. collects the candidates for the source name
2. filters candidates by call shape, explicit type/value arguments, and argument
   type compatibility
3. ranks surviving candidates lexicographically by conversion count, variadic
   use, a non-empty collector, fewer implicit copies into `var` parameters,
   parameter-signature length, then how a variadic candidate binds the rest of
   the arguments (more bound by value, then fewer `ref` parameters), and the
   generic/concrete tie-break. The copy a place costs a `var` parameter is
   charged to every candidate — a method's and a constructor's as much as a
   free function's — while the collector, by-value and `ref` terms decide
   between variadic candidates alone, since outside a pack the pinned Mojo
   follows different rules for a trivial value handed to a `var` parameter. A
   method is ranked by its own compile-time parameters, not its owner's. A
   string literal a type pack absorbs counts the conversion its
   materialization to `String` costs a regular parameter
4. accepts the unique lowest-score candidate
5. rejects no-match and tied-best cases

This first-pass ranking includes validated, nonraising user-defined `@implicit`
constructors. The checker records the uniquely selected converting constructor
at the source expression and MIR emits that call before its consumer. For
example, a typed `String` value selects `f(x: String)` over `f(x: Int)`, and an exact
`Int` argument selects `f(x: Int)` over a candidate requiring widening. A bare
integer literal passed to both `f(Int)` and `f(Float64)` is ambiguous because it
can materialize as either. Alpha-equivalent generic declarations are rejected,
while generic overloads with genuinely different bounds receive distinct lowered
symbols that include those bounds. Nested-def overload sets are rejected until
the lifting path can preserve their selected identities safely.

The important architecture point is that overload selection is static. The VM
does not inspect runtime value tags to choose between same-arity candidates.
The checker records the result on the checked node as `ResolveCallable` or the
full `SelectedCall` adjustment; recursively checked HIR carries it into MIR,
which preserves the selected callee through execution. A source-span map remains
only for compatibility queries and is not a semantic lowering input.

### Generic Implicit Conversions

Implicit conversion lookup first resolves the contextual target struct. For a
parameterized target such as `Box[Int]`, Mojito substitutes those checked
arguments into converting constructors before testing source compatibility and
constraints. It diagnoses ambiguity after substitution and records the selected
specialized constructor identity for MIR lowering. The same deterministic
ranking compares exact/coercing candidates, user conversions, variadic use,
the implicit copies a place costs a `var` parameter, signature length, and
generic/concrete specialization; the VM never repeats overload or conversion
selection dynamically.

### Borrow Checking In The Checker

The borrow checker currently lives with call checking because the checked
operation is local to one call expression.

For each argument, the checker classifies the operation:

- ordinary read/shared borrow
- `mut` or `ref` exclusive borrow
- consuming move via `^`

It then applies the mutable-XOR-shared rule by root/place. The checker is
place-sensitive enough to allow disjoint field borrows such as:

```mojo
f(mut p.a, mut p.b)
```

but reject conflicting uses of the same root/place such as:

```mojo
f(mut p, p)
f(mut p, p^)
```

This early borrow check complements, rather than replaces, MIR ownership
analysis. The checker handles local aliasing at call boundaries; MIR analysis
handles move state across control flow.

### Multi-Element Pointer Origins And View Loans

An origin-bearing pointer to a precise place designates exactly one value and
dereferences only at offset 0. A pointer origin **projected into an
interior-generation domain** (`origin._get_owned_interior["tag"]`, whether a
concrete place tail or a projected origin parameter) is the multi-element
form: offsets are legal (the VM's arena bounds check is the dynamic
backstop), the projection is part of the type so the discriminator also fixes
the runtime representation (allocation arithmetic, never a frame/slot
handle), and the origin's loan engages the ordinary interior-generation
staleness machinery. `unsafe_origin_cast` rebinds provenance without a runtime
operation (lowering forwards the receiver register; MIR verify compares
pointer ABI modulo origin) and never upgrades a statically immutable
capability.

Borrowed views (`Span`, `StringSpan`) are pointer-plus-length structs over
that capability. Their loans flow through three checked channels: a
constructor's `ref [origin]` parameter records `BorrowRefArguments` (the
binding loans each lent place), a view-typed subscript result records
`BorrowViewResult` (the result inherits the receiver's loans, falling back
to the receiver's own place for a plain owner), and `unsafe_origin_cast`/
`unsafe_offset` results carry their rebound or forwarded origins through the
aggregate-origin walks. The loans are whole-place and shared, so reads
coexist while structural mutation of the source conflicts with any live
view — including a `for ref` loop over the source, whose element loan is
mutable and therefore exclusive against the view (a value loop's element
loan is shared and coexists). A local annotated with an explicit origin
argument (`var v: Span[Int, origin_of(xs)]`) keeps that demand for every
later assignment, not only its initializer.

The **conservative subtree form** (`origin._subtree`, current Mojo's
experimental `Origin._subtree`) is a third origin shape beside precise
places and named interior generations: a terminal `OriginSeg::Subtree` on a
place path (or the `subtree` flag on a symbolic `Param`/`SelfPlace` pointer
origin, appended to the solved place after the interior tags). It
deliberately forgets which descendant of its base the pointer designates,
which drives every rule downstream. It is single-element (`multi_element()`
is false, offsets reject), the deref-place substitution refuses it
(`pointer_deref_place` — the runtime handle carries the true projection, so
the checked origin must never re-derive one), and for overlap the terminal
segment is a wildcard over every descendant. Its loan is a lazy interior
generation whose staleness predicate (`interior_origin_invalidated_by`)
drops both directional requirements the named-generation rule keeps: a
mutation base above, at, or below the subtree base invalidates it, and no
`Interior` segment is required below the mutation base. First-write
self-invalidation costs no new dataflow state: a pointer store records an
invalidation at the pointer's own source place with no protected handle,
that base carries the subtree tail, and because use-checking runs before
the transfer function the writing instruction still sees the live
generation while every later use observes the invalidation. The `except`
handle that ordinarily protects a reference from its own mutation is
carved out for subtree generations for the same reason. MIR verify accepts
a domain loan whose path ends in — but never continues past — the subtree
segment, and rejects subtree segments in transfer destination domains.

## Stage 4: HIR Lowering

Module:

```rust
crates/mojito-hir/src/hir.rs
```

Main type:

```rust
hir::Cfg
```

HIR is the first control-flow-aware representation. It is a region tree of
basic blocks, the blocks nodes of one `petgraph::StableGraph`, built by
walking the checked
statement and expression nodes (`CheckedStmtOp`, `CheckedOp`), as upstream's
control-flow lowering consumes typed ops. Expression-bearing instructions and
terminators carry `HirExpr`: a checked node, its operands' trees, and its
checked place; its type, category, effects, and adjustments are the node's own
attributes. Opaque statements (`HirStmt`) carry their statement node and their
expression roots' trees. A use names its definition rather than a spelling:
`Cfg::slots` records the slot each local-name node resolves to: its checked
binding's slot (`Cfg::bindings`), recorded where HIR declares the binding or
where an assignment to a name, whose target binding the check records on the
statement, first writes it, and otherwise the name's slot in HIR's lexical
scopes. A `try`'s parts are regions of the function's own walk
(`Lower::try_statement`), so a name in a part, read or assigned, is the
binding its scope declared, and an `except` region declares its handler
binding's slot in its own scope first (`HirExcept`). HIR retains
responsibility for statement control flow.

Statement control flow is structured, as upstream's parser and
`LowerSemanticCF` build `hlcf` ops. `Cfg::root` is the body's region: its
blocks, entry first. A run-time `if` is an `HirInstr::If` (`hlcf.if`) whose
first condition is its operand and each `elif` a condition region ended by
`Terminator::ElifCond`; every arm that completes ends in `Terminator::Yield`.
A `while` or `for` is an `HirInstr::Loop` (`hlcf.loop`) labelled uniquely in
its function, as `LowerSemanticCF` numbers loops: its body opens with an
`If` on the condition, or on the `Bool` slot the iterator protocol wrote
(`HirCond::Slot`), whose else arm holds the loop's `else` body and then
`Terminator::Break(label)`; the body ends in `Terminator::Continue(label)`.
The `else` body lowers outside its loop's frame, so its own
`break`/`continue` name the enclosing loop (`lowerLITLoop`). A `comptime for`
is an `HirInstr::ComptimeFor` (`hlcf.comptime.for`), left with
`ComptimeBreak` and iterated with `ComptimeContinue`; its `orelse` is the
stop arm, ended by `ComptimeBreak`, which MIR lowering puts in the else arm
of the `ComptimeIf` on `has_next` the op's body opens with, as
`LowerSemanticCF` leaves upstream's body. The elaborator unrolls the body
N+1 times, the exhausted iterator's copy keeping only the stop arm. A `comptime if` is
an `HirInstr::ComptimeIf` (`hlcf.comptime.if`): its conditions, computed
first as upstream's parser does, then one arm region per condition and an
else region, each arm that completes ended by `Terminator::ComptimeYield`.
A `match` is an `HirInstr::Match` (`hlcf.match`): one region per case,
holding its tests as nested `If` ops whose failing arms end in
`Terminator::MatchNext`, its bindings, its guard, and its body, ended by
`Terminator::MatchComplete`, then an else region ended by `Yield` for the
fall-through when no case matched (enum-case and sequence patterns land by
roadmap entry, R1132–R1140).
No HIR terminator jumps between blocks, so `Cfg::g` holds no edge. HIR
lowers only a body's live statements (`CheckedTables::live_statements`):
the statements after a `return`, `break`, `continue`, or `raise` are erased
before they lower, as `LowerSemanticCF`'s `eraseOpToEndOfBlock` erases the
ops after a terminator, so they reach no slot, block, or nested function
(`find_nested_defs` reads the same rule). MIR lowering builds the matching
MIR region ops (`Flatten::hir_region`), and `lower-control-flow` lowers them
to blocks at the end of `lower_cfg`.

Each HIR block has:

```rust
pub struct BasicBlock {
    pub instrs: Vec<HirInstr>,
    pub term: Option<Terminator>,
}
```

Each block is sealed with exactly one terminator:

```rust
pub enum Terminator {
    Return(Option<HirExpr>),
    ReturnWithCleanup { value: Option<HirExpr>, cleanup: Vec<VarId> },
    TryYield,
    Yield,
    ComptimeYield,
    ElifCond(HirExpr),
    Break(u32),
    Continue(u32),
    ComptimeBreak,
    ComptimeContinue,
}
```

Iterator-driven loops use `ReturnWithCleanup` when a source `return` bypasses
their common exit. The return expression is materialized first, then MIR keeps
the current loop binding and synthetic iterator owners alive into the return's
block, where drop elaboration ends them.

The core invariant is:

> Every block has one terminator, and terminators and region ops own the
> control-flow shape.

That invariant makes later MIR and analysis passes structure-driven rather
than syntax-driven.

### Variable Slots

HIR also interns variables into stable `VarId`s.

```rust
pub type VarId = u32;
```

Function parameters are seeded first, so parameter slots are stable:

```text
vars[0..n_params]
```

This becomes the VM call ABI later. A callee frame receives argument values by
writing them into the first `n_params` variable slots.

A free-function named `out` result is deliberately outside that ABI prefix. MIR
seeds it as an uninitialized callee-local slot, rewrites fallthrough and bare
returns to read that slot, and exposes its declared type as the function result.
The caller therefore invokes a named-result function exactly like an ordinary
returning function and never supplies the `out` argument.

A `var x: T` without an initializer declares its slot (`HirInstr::Declare`,
carrying the checker's declared type as the statement's `binding_ty`) with no
def, as upstream allocates the storage without a value. MIR records the slot's
type and binding and emits `DeclareVar`, the slot's declaration point: it is
no liveness def or use, but drop elaboration's region locality counts it as a
def site, so a slot declared outside a `try` and first assigned inside it
survives the body's exit, while one declared inside the body is still in the
body's `Try.cleanup`. Ownership analysis starts every
non-parameter slot uninitialized (see the store-drop flow under drop
elaboration), so the first assignment initializes the slot and destroys
nothing. The checker's `Uninitialized` marker has no value: lowering it as an
expression is an explicit `Unsupported`.

Trailing `where` clauses compile once into the checked `GenericConstraint`
algebra. The `(condition, "message")` form becomes a `WithMessage` wrapper:
evaluation, implication, conformance assumptions, and specialization-closure
checks recurse through the semantic condition, while a failed call reports the
retained message. This keeps diagnostics attached without making message text
part of generic identity or proof semantics. The same wrapper is retained on
struct declarations, conditional conformances, method availability, associated
and trait comptime members, and comptime declarations.
Associated-member constraints are validated at concrete projection; a
conditional-conformance failure feeds its retained reason into lifecycle and
trait diagnostics. Origin-mutability-only function constraints survive the
ordinary generic erasure in `CallableOriginSignature` and are discharged after
call-origin solving recovers the inferred Bool binding. The checked constraint
contract is plural: every declaration family stores one compiled constraint
per trailing `where` clause, the first failing clause reports its own message,
and truth-only operations (implication, inherited-requirement merging) fold a
clause list into one conjunction without erasing the stored per-clause
messages. Per-trait conditional-conformance conditions stay single-clause.

Evaluation is three-valued. `Checker::constraint_verdict` lowers a constraint
under an environment to a `Bool` parameter expression and classifies it as
`Proven`, `Disproven`, or `Residual`: a leaf the bindings decide is a constant,
an arithmetic operand (`ConstraintOperand::Expr`, the `n + 1` of `where n + 1
== m`) stays its canonical expression, and a leaf whose parameter has no
binding is unknown. The connectives are the parameter-expression context's, so
`not` over a residual is a residual and a missing binding never becomes an
acceptance under negation. As at the pin, an application needs evidence even
with its arguments symbolic: a residual passes only when the enclosing
declaration's own `where` assumes the same canonical proposition, and is
otherwise reported as lacking evidence rather than as false. A selecting
consumer (an overload, a conditional conformance, method availability) reads
`eval_generic_constraint`, which is `is_proven()`.
Generic top-level comptime aliases lower once into the checker's alias
registry (classified `ParamDecl`s plus a symbolic template shared with
parameterized associated members) and expand per application in type
resolution through the same `resolve_use_params` contract as a struct
application; an alias is a pure type declaration with no runtime form, skipped
at MIR program assembly like a struct or trait.

A variadic type parameter retains a leading `*` in the checked parameter name.
Generic-call inference recognizes the matching `*args: *Pack` element type and
checks each overflow argument independently against the pack bounds instead of
forcing all arguments to unify to one type. Specialization records the concrete
sequence as the internal checked `TyKind::RuntimePack([T0, ...])` call ABI; the VM
materializes that collector in private `Value::Tuple` storage. An ordinary
homogeneous `*args: Tuple[...]` instead remains a nominal `List` whose repeated
element type is the public nominal `Tuple`. This supports heterogeneous calls
and pack length queries without confusing a public tuple with the private
`TyKind::Tuple` carrier. Specialization infers literal
and directly constructed call-argument types, binds the pack's type tuple into
the compile-time environment, and unrolls `args.__len__()`-driven loops.
Because specialization consumes this generic call before whole-program checking,
it queries a declaration-only checker conformance oracle first. Every inferred
element is tested against every declared pack bound at the requesting call site;
a failure names the one-based element number, concrete type, pack, and trait.
The oracle reuses the authoritative conformance rules and records trait
refinement, nominal/conditional conformances, field types, and lifecycle-method
presence without checking method bodies early. Full conformance verification
still runs on the elaborated program.
A homogeneous collector (`*args: T`) is never specialized by arity: the
elaborator unifies each overflow argument against the element type only, so
one instance per element type takes every call's pack at run time as
`VariadicPack(T)`, as the pin's `VariadicList` does. The VM carries it as a
private `Value::Tuple`; native code passes upstream's element-pointer
descriptor (`docs/native-abi.md`). A run-time pack held by its owned
parameter or by a consuming iterator over its rest destroys its elements
last to first, as `VariadicList.__deinit__` does.
Each unrolled static index substitutes its concrete element type while retaining
the declared common bound for operations that are not specialized to one index.
Pack expansion necessarily runs before checked `OwnerId`s exist, so it owns a
small lexical resolver with private monotonic `ElabBindingId`s and independent
value/type namespaces. A runtime pack is identified by its specialized
`$pack[T0, ...]` parameter declaration and its length is keyed by that binding;
concrete type-pack substitutions are likewise keyed by the specialization's
type binding. The walker mirrors declaration order and the lexical scopes of
branches, loops, handlers, comprehensions, nested definitions, generic
parameters, and individual struct methods. A same-spelled local therefore gets
a different identity, leaving an unsupported ordinary spread for the checker,
while leaving the scope restores the outer pack. Presence in the binding map —
not a nonzero length — identifies an empty pack. HIR loop lowering follows the
same rule by assigning the loop target a scoped runtime slot instead of
reusing/leaking an outer same-named slot.

A nested declaration is not specialized above MIR. A nested `def` or lambda
reads its enclosing declarations' value parameters as parameters, not
captures, and names their binders in its types and compile-time control flow;
the elaborator instantiates it under each enclosing instance, and a generic
one once per call from the call's solved arguments
(`MirInstr::CallIndirect::instantiated_args`), beside the inherited ones.
A nested declaration inside a `comptime for` is a generator over the loop's
index as well: each unrolled copy of the region points its closure at the
body's instance under the copy's iteration, and a generic one carries the
copy's bindings to its calls.

Pack forwarding first flattens the one known spread into a virtual positional
type sequence and runs the shared call-slot matcher; only positional overflow
becomes the target pack's inferred type list. The spread must follow every fixed
positional argument, while parameters after the variadic collector are supplied
as keywords or defaults. Specialization gives that target instance a regular
private runtime-pack collector slot and moves the complete collector as one
value. This
preserves linear elements without inventing an illegal move through `args[i]`.
Both the top-level and lexical nested specialization passes reject a second
spread, explicit positional overflow after the spread, or a non-pack target;
those are current-Mojo rejections, not deferred concatenation features.

A variadic **struct** template (`struct S[*Ts: Bound]`) is specialized the same
way: compile-time elaboration keeps the template verbatim, resolves every
explicit instantiation (calls, `TypeApply` expressions, and type annotations) to
a mangled fully concrete struct, resolves pack-applied member annotations such
as `Tuple[*Ts]` to the corresponding concrete nominal specialization, rewrites
a pack-typed method
parameter to the `$pack` element list, unrolls current Mojo's dependent
accessor `__getitem_param__[i: Int] -> Ts[i]` into per-element concrete methods,
and drops the template. The earlier `__getitem__` spelling follows the same
path only as an explicit compatibility fallback. Every specialization reuses
the template's spans (correct
provenance), so struct annotation sites are identified by the struct's unique
name and each specialization's subtree — and each unrolled accessor body — is
stamped with a distinct source tag, keeping span-keyed checked facts separate.
If an element is a function value, its exact checked `Func`/`GenericFunc`
contract cannot be losslessly respelled as a source `def(...)` annotation:
defaults, variadics, generic declarations, capture origins, and typed errors are
richer semantic facts. Generated Tuple AST therefore carries only an opaque,
parser-unconstructible callable-type id. The compiler seeds the final checker
pass with the corresponding `Ty`; the source AST never embeds or reconstructs
that semantic type.
The checker resolves a subscript on such a struct at a compile-time-constant
index and records its complete selected accessor contract. MIR `Index.call`
carries the resulting `MirSubscriptCall`, and the VM dispatches ordinary value
reads, explicit reference bindings, and chained receiver/place uses without
name or effect derivation.

A non-pack struct may also define `__getitem_param__[i: Int]`: subscript
checking supplies the source index as a compile-time value parameter, retains
the exact generic-method target, and reuses the already evaluated source-index
register for the value-parameter ABI. This is separate from runtime
`__getitem__(index)` dispatch and does not add an ordinary positional argument.

The implicit prelude exposes `List`, `Set`, `Dict`, `Optional`, `Range`, and
`Tuple` as ordinary bundled structs (upstream requires an explicit import for
the `Set` name — a recorded divergence pending display-lowering identity
plumbing). List, set, and dictionary displays lower to the
selected nominal constructor; comprehension leaves lower to ordinary
`append`, `add`, or `__setitem__` calls. Range syntax is an ordinary overload of
the bundled `range` function. Tuple displays request a concrete specialization
of `Tuple[*Ts]`, whose private `__RuntimeTuple[*Ts]` field is the only source
construct that lowers to `MakeTuple`. Indexing, sizing, containment, comparison,
reversal, concatenation, and tuple-element consumption therefore use checked
methods just as they do on user structs. List, Set, Dict, and Range borrowed
iteration likewise follows selected nominal methods; consuming bundled
iteration is currently List-specific. Public Tuple has no runtime `__iter__`
contract.
The checker owns the implicit-copy decision in one funnel
(`Checker::check_consuming_as`): a *place* reaching a consuming position —
`var`/`deinit` parameters and receivers, operator `var` operands, storage
initialization, assignment, field stores, returns, displays — is an implicit
copy only when its type is `ImplicitlyCopyable`, and MIR receives
`CopyPlaceValue` only for such places; a `Copyable`-only place must spell `^`
or `.copy()`, exactly as upstream, and there is no last-use move. Conditional
lifecycle conformances are folded per specialization: a `deinit` method may
implicitly copy a named tuple only when every element is `ImplicitlyCopyable`;
otherwise the call must transfer the tuple with `^`. A borrowed `self` method
receiver over a reference result reads its retained place — `MethodCall`
records `recv_writes` for a `mut`/mutable-`ref`/consuming receiver — so a live
shared loan on that place does not conflict with the call.

Borrowed-source iteration lowers uniformly, in statements and comprehensions
alike. The checker records a borrowed origin for a named source — a concrete
List/Set/Dict place gains an `element` `Interior` segment, while a named
user-struct iterable records the *whole* source place — and HIR `BorrowIter`
preserves that place instead of invoking its value copy lifecycle. MIR always
binds the retained source as a genuine reference (`MakeRef` into a
`TyKind::Ref`-typed retained-source slot; a borrowed `__iter__(ref self)` re-roots
at the source, so the handle is read only by `GetIter` and dropped afterward
as a no-op) and normalizes the iterator into a distinct iterator-object slot.
Whole-source versus interior borrowing is expressed only as loan granularity:
`EstablishLoans` carries either a whole-place shared loan or an interior
`element` generation, re-established on the long-lived iterator-object slot so
the source stays live through the loop and mutation of it during iteration is
rejected. List iteration additionally observes element replacement and rejects
structural invalidation before a later iterator use; `for ref` binds the
yielded reference handles of the ordinary protocol, with the iterator's source
loans re-established on each binding. The remaining collection-specific
checker rules (the interior-`element` attachment and the mapping snapshots)
stay narrow until generic-bound origin derivation lands with mapping
invalidation. The only method-free
collection behavior in the VM is the explicitly CTFE-only `ComptimeList`
bridge; the separate method-free tuple-shaped path is compiler-private
runtime-pack storage, not a public collection.

The public `Tuple` is an ordinary struct generator. `std/builtin/tuple.mojo`
declares one template, the check types it once with `*Ts` symbolic, and
`native::mono` mints an instance per element list. A tuple's type is the
nominal `Tuple[T0, ...]` applied element by element
(`types::canonical_pack_arguments`); a display types its literal elements as
literals, and the instance is named by their materialized types
(`mono/unify.rs:bind_pack`). `t[k]` is `__getitem_param__[k](ref self)`,
whose reference result is the element's place, and a temporary tuple lends
it from a hidden slot. The two transforms are declared with upstream's
signatures: a named `out` result typed by a spread of a list computed from
the element packs, `Tuple[*Self.Ts.reverse()]` and
`Tuple[*TypeList._concat[Self.Ts.values, OtherTs.values]()]`, which the
checker carries as `ParamKind::ListTabulate`/`ListConcat` and the elaborator
closes per instance. Each consuming member moves its elements out with
`Pointer(to=self[i]).unsafe_take_pointee()` inside a `comptime for` the
template keeps and then ends the storage with `MirInstr::MarkDestroyed`,
since the private storage would otherwise destroy the moved elements again.
`TString` is a generator the same way. The checker writes a `t"…"`
occurrence in one pass, as the pin does, as the call of the bundled entry
point `__make_tstring` (named by its linked module path, never looked up in
user scope) and records it as `SemanticAdjustment::SpelledConstruction`, the
adjustment an initializer list's construction shares; the checked arena and
HIR hand MIR the call in the literal's place, so MIR has no t-string form.

### If

An `if`/`elif`/`else` chain lowers to a diamond or chain of diamonds:

```text
current -> branch
          /      \
       then      else/next-elif
          \      /
           join
```

Branches that already returned, broke, or continued are sealed, so the lowerer
does not add spurious join edges.

### While

A `while` lowers to:

```text
preheader -> header -> body -> header
                    \-> exit
```

`break` targets `exit`; `continue` targets `header`.

### For

A `for` lowers to the same control-flow shape as a `while`, with explicit
iterator protocol instructions. Current typed-raising iterators call
`__next__` exactly once per trip and treat only the checked `StopIteration` type
as normal exhaustion:

```text
bind iterator
initialize iterator through selected __iter__ chain
header:
    try_next(iterator) -> (loop variable, yielded)
    branch yielded, body, exit
body:
    user body
    jump header
exit:
```

A nonraising `__next__` has no loop shape: the checker rejects it with upstream's
`__has_next__` diagnostic, and only compiler-private carriers (the CTFE
`ComptimeList` and an owned runtime pack) test `has_next` before a method-free
`next`. A borrowed loop over a named homogeneous runtime pack yields element
references, as upstream's `VariadicListMem.__iter__` does
(`IterationProtocol::indexes_pack`): the pack is borrowed whole through
`BorrowIter`, and MIR inlines upstream's `_VariadicListMemIter` as a
`PackCursor` — the iterator slot holds the pack's handle and loans, an `Int`
index slot counts the yielded elements, `has_next` compares it with the
pack's run-time `len`,
and `next` is a `MakeRef` of the pack element at the index. No element is
copied or moved out.
For a concrete borrowed List, Set, or Dict place, the preheader carries its
checked interior origin through `BorrowIter`; `for ref` binds the protocol's
yielded reference handles directly — the former List-only indexed-place
desugaring is gone.

Concrete iterator selection retains an exact checked `__next__` operation:
target, raising effect, executable result type, and (when present) the
origin-bearing reference result. HIR and MIR carry that contract unchanged, and
MIR verification checks it against the selected declaration. Thus a
reference-yielding iterator writes a `TyKind::Ref` register and binding; the VM is
never relied on to smuggle a reference handle through a register typed as its
referent. Compiler-private iterator carriers alone omit the nominal operation
contract.

Abstract iteration instead retains the value type promised by the trait. Its
checked `TryNext` operation, which catches the `StopIteration` that the bundled
`Iterator.__next__` requirement declares, carries the same explicit copy-reference
adapter as an ordinary bounded `iterator.__next__()` call. Runtime retargeting
leaves a concrete value return unchanged, or materializes and lifecycle-copies a
concrete reference return before writing the value-typed destination. A
reference into the iterator receiver is read from the identified just-completed
callee frame before that temporary frame storage is discarded.

### Try Regions

`try` is represented structurally rather than fully inlining all exceptional
edges into the surrounding CFG.

HIR emits an `HirInstr::Try` that carries the original `try` statement plus a
snapshot of the enclosing loops (`EnclosingLoop`: the label or `comptime for`
a `break`/`continue` names, and a return's cleanup). This is needed for
source like:

```mojo
for i in range(10):
    try:
        break
    finally:
        print(i)
```

The `break` targets a loop outside the `try` region. A seeded try-region CFG
lowers it to `Terminator::Break(label)` naming that loop, and
`lower-control-flow` resolves it to `MirTerm::EscapeJump { target }`, where
`target` is a block of the function's list, not the region's. The VM
propagates this as a non-local jump while running `finally` blocks on the
way out. A `break` from a `try` part to a loop inside an enclosing `try`
part reaches the unsupported boundary there.

## Stage 5: MIR Lowering

Module:

```rust
crates/mojito-mir/src/mir.rs
```

Main entry points:

```rust
lower_cfg(cfg: &hir::Cfg) -> MirFunction
lower_program(program: &[Stmt]) -> Result<MirProgram, TypeError>
lower_checked_program(program: &CheckedProgram) -> MirProgram
```

Normal compilation uses `lower_checked_program`. The compatibility
`lower_program` entry point performs semantic checking and propagates any
`TypeError`; it never manufactures unchecked semantic data. VM-backed CTFE
fragments also pass through `check_program`; the former source-type approximation
path has been removed.

MIR is the stable waist of the compiler. HIR still has nested expressions; MIR
flattens them into A-normal form / three-address code.

For example:

```mojo
foo(bar(x + 1))
```

becomes a sequence of register-producing instructions:

```text
r0 = use x
r1 = const 1
r2 = r0 + r1
r3 = call bar(r2)
r4 = call foo(r3)
```

Every intermediate value gets a virtual register:

```rust
pub struct Reg(pub u32);
```

Every variable remains a `VarId` slot:

```rust
pub type VarId = u32;
```

The VM frame has both:

```text
regs: Vec<Value>
vars: Vec<Value>
```

Registers hold temporaries. Variable slots hold source-level locals,
parameters, and synthetic locals such as iterators.

### MIR Program Shape

A lowered program contains:

- one synthetic `__toplevel__` function
- one `MirFunction` per top-level `def`
- one `MirFunction` per lowered struct method
- lifted nested functions where the compiler can safely lift them

Production compilation rejects executable file-scope source statements. The VM
runs the synthetic module-initialization function and then calls zero-argument
`main()` if it exists; the same synthetic function supports explicitly opted-in
legacy statement snippets in phase-level tests.

### MIR Blocks And Terminators

MIR blocks are simple:

```rust
pub struct MirBlock {
    pub instrs: Vec<MirInstr>,
    pub term: MirTerm,
}
```

Terminators are:

```rust
pub enum MirTerm {
    Jump(MirBlockId),
    Branch { cond: Reg, then_b: MirBlockId, else_b: MirBlockId },
    Return(Option<Reg>),
    ReturnWithCleanup { value: Option<Reg>, cleanup: Vec<CleanupDrop> },
    TryYield,
    EscapeJump { target: MirBlockId, cleanup: Vec<CleanupDrop> },
}
```

A cleanup list is a `Vec<CleanupDrop>`: each entry names its slot and the
destructor the drop runs, as a `DropVar` does, so the backends call it
without looking it up from the slot's type.

`TryYield` (`lit.try.yield`) completes a `try` part and never ends a function
body. `EscapeJump` is for a `break`/`continue` inside a try region whose
target belongs to the enclosing function. Drop elaboration writes
`ReturnWithCleanup` on a return that crosses a `finally`, listing what the
return still holds once the crossed `finally` blocks have run; the VM runs
every pending `finally` before destroying those values and completing the
return.

The continuation-driven VM frame path binds reference parameters directly to
caller frame/slot handles. Structured `try` sub-regions execute calls
synchronously, so the VM temporarily pushes a mirror containing the retained
caller's real `FrameId`, registers, and variables. Direct, indirect, method,
callable-struct, and constructor calls can then use the ordinary handle ABI:
mutations update the mirror even when the child raises, and projected or
aggregate reference returns already name caller storage. The mirror is copied
back and removed on every outcome; there is no separate result-rebasing rule.

### Places

MIR separates rvalues from writable places.

```rust
pub struct MirPlace {
    pub root: VarId,
    pub root_ty: Option<Ty>,
    pub proj: Vec<Proj>,
    pub projection_tys: Vec<Ty>,
    pub ty: Option<Ty>,
    pub through: Option<VarId>,
}

pub enum Proj {
    Field(String),
    Index(Reg),
    ConstIndex(usize),
    Variant(usize),
}
```

A place is something that can be read, written, moved from, or borrowed:

```text
x
p.field
p.items[i].x
xs[i]
```

This is one of the key architecture choices. Mojo-like ownership and borrowing
need to know the difference between "the value computed by an expression" and
"the storage location this expression names." MIR makes that difference explicit.

Production checked lowering requires `root_ty`, one result type per projection,
and the final stored `ty`; the optional wrapper exists only for the deliberately
unchecked phase-test API. HIR carries the same information earlier as
`HirPlace`: stable `OwnerId`, root type, typed field/index projections, and
final storage type.

Dynamic `Index(Reg)` projections conservatively overlap every index. A
`ConstIndex` is emitted only for an exact nonnegative literal selecting an
element of compiler-private heterogeneous `TyKind::Tuple` storage. This gives each
pack element a distinct ownership path without changing nominal collection
subscript dispatch. A store to such an element is a store to that path: an
`out self` initializer builds the storage one element at a time
(`self.storage[i] = Self.Ts[i]()`), the checker counts the field
initialized once every element is stored, and the VM's uninitialized `self`
holds one placeholder per element for the stores to fill.

When an accessor returns a reference used immediately as another receiver or
place — including the target of a field store or augmented assignment
(`xs[i].field = v`) — lowering evaluates it once into a hidden `TyKind::Ref`
local. `DefVar` stores the handle and `EstablishLoans` retains its owner
generation; the derived place names that local in `through`. Later chained
loads, projections, stores, and calls therefore preserve provenance without
rerunning the accessor or treating its referent as owner storage; a nominal
subscript never becomes a raw `Index` projection on the collection.

Every register is typed. Expression results record their checked type as they
lower; synthetic registers (handles, markers, short-circuit and iterator
temporaries) are typed at their emission site; and a `close_register_types`
pass fills the remaining results by copying facts already present in the
instruction stream — operand register types, place storage types, inline
element types, slot types, and declaration returns — never by re-implementing
checker inference. Loan and consumption markers, which hold no runtime value,
are typed `TyKind::None` by convention. Functions additionally carry their checked
`ret_ty`, raising contract (`raises`/`error_ty`), and per-slot `var_tys`.

`mir::verify` has two modes. `verify` accepts parametric MIR, the form the
canonical artifact and the elaborator's input take: a register, slot,
signature, place, or instruction type may name a parameter expression — a
type binder, a dependent type, a symbolic vector lane or width, a symbolic
struct argument, a compile-time application — and `verify/scope.rs` checks
that every binder it names is in scope (the function's declaration, a
method's struct, a generic callable signature enclosing the occurrence, or a
contract binder spelled by the contract or witness request that holds it)
and that every expression is well-kinded for its slot
(`docs/notes/param-expr-attributes.md` §Register types). `verify_concrete` is for
concrete MIR, the form the VM and the native backend run: it applies the same rules and then rejects what only
parametric MIR may carry (`verify/concrete.rs`) — a symbolic type in any
signature, slot, register, place, instruction, or declaration; a compile-time
parameter a declaration or a call contract still declares; a layout query
(`SizeOf`), which the elaborator answers under the compilation's native
target; a `ConstructTypeParam`; and a reference slot read as its handle,
a whole-root `LoadPlace` of a slot typed `ref T` (`T` not itself a
reference) whose register is a reference, or a `UseVar` copy of such a
slot. A copy-reference result adapter survives elaboration
only on a `__next__` step that returns a reference, where it copies the
element out; the elaborator drops it from a step that returns the element.
Structured control flow is checked in both modes: each region terminator
sits in the region of the op it ends, with the op's result arity and types;
a `break`/`continue` names an enclosing loop; a `comptime.for.break` or
`continue` has a `comptime for` as its nearest loop (`verify/structure.rs`).
The slot-ends check runs on a body's `lower-control-flow` lowering. A third
mode, `verify_structured_function_in`, adds upstream's structured form for
a body: every region, the body included, one block, and no CFG terminator
(`Jump`, `Branch`, `ComptimeBranch`, the `ComptimeFor` terminator,
`EscapeJump`); its shape half, `verify_structured_shape`, runs in
production on every body `lower_cfg_nested` builds, before
`lower-control-flow`, each finding a MIR invariant error. `verify_concrete`
rejects every structured op and region
terminator, which the transitional lowering removes first.
`native::mono` verifies its output in the concrete mode, and asks
`concrete_function_findings` per instance so that a failure names the
instance it came from. The concrete mode also rejects a declaration that
still carries an availability clause or a conformance condition.

A struct member's `where` clause is declaration metadata:
`MirFunctionDeclaration.availability`. What decides it rides with it.
`MirStructDeclaration.declared` holds the struct's declared conformances,
each with its condition over the struct's binders;
`MirStructDeclaration.conformances` holds the struct's synthesized answer
for each lifecycle trait and `IsTrivially*` spelling
(`mojito_types::conformance::LIFECYCLE_TRAITS`), which depends on its
methods and fields as well; and `MirDeclarations.traits` holds each declared
trait with its parents (a package's, through `trait_declaration`, decoded on
first lookup). The checker states these facts once per declaration
(`Checker::conformance_facts`), the package build for every library struct
and an importer's check for its own, and answers no struct for a trait
ahead of use. The elaborator decides `conforms_to(S, X)` when a clause asks
it, under an instance's bindings (`mono/availability.rs`), by the rule the
checker shares (`mojito_types::conformance::struct_trait_rule`): a declared
trait from the declared conformances that are `X` or refine it, a lifecycle
trait from its row, an operation trait never, and any other compiler-known
trait always. A compiler-modelled type that is not a struct (a `SIMD`
scalar, a tuple, a pack) conforms to a library-declared trait only as the
upstream struct it stands in for lists it
(`mojito_types::conformance::modelled_declares`): all of them `Sized`. It ranks and re-derives nothing. A member its instance disproves is skipped when struct
discovery reaches it and is an error when a call demands it. An instance that
fails to materialize fails the compilation. The text schema carries the three
fields from 1.9, and an older artifact reads as carrying none. Schema 1.10
adds the `sig_carried(type)` signature origin; schema 1.42 adds a struct's
`declared` list and each trait's parents, an older artifact's trait reading
as parentless.

A struct's unparameterized associated types ride the same way:
`MirStructDeclaration.associated_types` holds each member's type over the
struct's own binders (`ConformanceFacts::associated`, text schema 1.12). The
elaborator unifies a signature's `C.Element` against a call's types first
(`mono/unify.rs`); one no signature spells it substitutes from the member of
the struct `C` is bound to (`declared_associated_type`,
`mono/substitute.rs`). A parameterized member rides in
`MirStructDeclaration.parameterized_associated_types`
(`ConformanceFacts::parameterized_associated`, text schema 1.46): its
template plus its explicit parameters, each keyed as the template names it.
`declared_associated_type` binds the application's origin arguments into
the template (`mojito_types::types::substitute_origin_params`, which the
checker's `substitute_assoc` shares) and its type arguments as binders,
then substitutes under the instance. Elaborated MIR carries no associated
types.

A loop or comprehension whose iterable has an `__iter__` step, borrowed or
owned, splits its source from its iterator, as upstream's iterator is a
value of its own type: it declares the iterator slot (`$iterobj<N>`,
`$compiterobj<N>`) at the `GetIter` that declares it, and an owned
`__iter__(var self)` consumes the source slot
(`MirInstr::consumed_iteration_source`), which no drop names again. The
slot is typed with `IterationProtocol::iterator_ty`: the checked
`__iter__` result, its `origin_of(self)` resolved to the named source
(`Checker::resolve_iterator_origin`), symbolic in a generic body
(`C.IteratorType[origin_of(xs)]`, read off the bound's own `__iter__`).
`mir::verify` rejects a `UseVar` or `DropVar` slot with no type, and
`rewrite_get_iter` keeps a split slot's declared type, typing only a source
normalized in place (R664).

A struct's compile-time value members ride beside them:
`MirStructDeclaration.associated_values` holds each member's value as a
parameter expression over the struct's own binders
(`ConformanceFacts::associated_values`, text schema 1.36). A read of a
member through a type that is still a parameter (`T.K`, `Self.T.K`) is a
`ParamKind::TypeMember` node, upstream's member lookup on a type the
elaborator binds; the checker records it as `SemanticAdjustment::ParamValue`
(`Checker::read_type_member`, `type_member_value`), MIR carries it as
`Const::Param`, and `native::mono` answers it per instance
(`ParamContext::answer_type_members` with `type_member_answer`,
`mono/symbolic.rs`), evaluating the member under the bound instance's own
bindings. A read on a closed type (`A.K`, `B[4].K`, `Self.K`) folds in the
checker. A parameterized value member, upstream's generator, rides in
`MirStructDeclaration.parameterized_associated_values`
(`ConformanceFacts::parameterized_values`, text schema 1.80): its body over
the struct's binders and its own parameters. A read through a bound
(`T.get[0]`) is a `TypeMember` node carrying the arguments the checker
bound against the trait's requirement, which `type_member_answer` binds to
the conformer's parameters by position before evaluating the body under
the instance. Elaborated MIR carries no value members.

A generic `def` call carries the compile-time arguments the checker solved,
in declaration order and in the caller's binder scope
(`MirInstr::Call::instantiated_args`, text schema 1.13, from
`SemanticAdjustment::InstantiatedArguments`). The elaborator binds the
callee's own type parameters from them before unifying the runtime
arguments, so a parameter no runtime parameter or result spells is bound
too. Elaborated MIR carries none.

`mir::verify` is the standalone semantic verifier of record. From MIR plus
`MirDeclarations` alone it checks place completeness and projection
consistency, register bounds and register-type completeness, store/binding/
return/call-argument type consistency (through the checker's coercion
predicate — never re-derived rules), CFG-edge validity (jump-target bounds per
region, `TryYield`/`EscapeJump` only inside `try` sub-regions), effect
protection (a raising site in a nonraising function must sit under a handler),
and reference invariants (`StoreRef` targets reference storage; declared
write-back parameters receive caller places). For method-dispatched nominal subscripts it also
verifies the selected target against its declaration, exact positional/keyword/
default and variadic source binding, operand and collector types, generic value
arguments, receiver and argument place requirements, capture slots,
checker-selected executable result (including exact reference origin and
permission), and protection of every raising subscript form. Function
declarations retain whether a receiver exists, its declared convention, aligned
explicit-parameter conventions, positional- and keyword-variadic collector
conventions, and the reference-return ABI. The verifier permits only declared
`Ref` to effective `Read` narrowing. An abstract trait-bound target has no
concrete body declaration until runtime retargeting; its complete selected
requirement is therefore retained as typed call-local MIR metadata (including
argument conventions/place requirements, result ABI, effects, generic
arguments, and any result adapter) and verified as the abstract declaration of
record. Concrete targets are additionally checked against `MirDeclarations`. The
pipeline composes it with
`analysis::check_ownership_program`, which owns the ownership dataflow; the
compiler rejects findings as `CompilerError::Verify`, and the VM re-verifies
the drop-elaborated program it actually executes. The loan analysis reads
each call's retained argument places against the callee's declaration
(a program-wide `CalleeRefParams` table): a place retained at a slot whose
`param_writes` mask is set — a `mut` parameter, or a `ref` whose declared
origin is statically mutable — is an exclusive write, while a place lent to
any other slot — a read convention, or a `ref` under an immutable,
parametric, or bare contract, which no body can write through (a
borrowing-view constructor, `Named`) — is a shared read. A single-`__init__`
construction calls its bare struct name and resolves to the
`<name>.__init__` declaration; a callee without a declaration falls back to
its function's `ref_params` mask, and an unknown callee stays exclusive.

A place's storage type is distinct from its expression value type. For a field
declared `ref[origin] T`, the place stores `TyKind::Ref`, while an ordinary load
produces `T`. The VM now chooses reference read/write behavior from this checked
place type; it does not inspect a runtime `Value::Ref` to rediscover semantics.

### Important MIR Instructions

An instruction is sized for its common variants: the optional places,
subscript calls, and call-only types that only a few call variants carry sit
behind boxes, and a compile-time assertion beside `MirInstr` pins its size.
Boxing changes no encoding, since serde writes a box as its contents.

Representative instructions:

```rust
Const
SizeOf
ParamListAddress
TypeName
Rebind
UseVar
MovePlace
DefVar
UnOp
BinOp
Call
MethodCall
GetField
Index
Slice
MultiIndex
MultiSet
Store
LoadPlace
MakeTuple
MakeVariant
VariantIs
VariantGet
VariantSet
VariantSetInitWith
VariantTake
VariantDeinitWith
VariantReplace
MakeSimd
Raise
Try
DropVar
HasNext
Next
Unsupported
```

`ParamListAddress` is the static address of a parameter list's elements,
the bundled `ParameterList.get_span`'s stand-in for upstream's
`global_constant`: a template names the list as a parameter constant, the
elaborator closes it to a tuple, and each backend lays the list out once per
program (a never-freed VM allocation, a private constant global natively).
A value pack read at run time reaches it as the
`ParameterList[values.values]()` construction the checker spells in the
read's place, over the pack's raw list `KGENParamList[T]`, a compile-time-only
type no elaborated register, slot, or signature may hold: a runtime subscript
calls `__getitem__` and a runtime length `__len__`, even at a compile-time
index. A compile-time length is the struct's `size` member, read through
the value (`values.size`), whose body the bundled struct alone spells over
its raw list.

Public collection displays and comprehensions use ordinary `Call` and
`MethodCall` instructions. `MakeTuple` is reserved for the compiler-private
heterogeneous pack behind `__RuntimeTuple`; constructing the public nominal
`Tuple` uses an ordinary constructor call.

Every method-dispatched nominal subscript carries a `MirSubscriptCall`
containing the exact target,
typed raising effect and executable result type, receiver place requirement and
effective convention,
source-bound argument types/conventions/places, capture accesses,
reference-result origin, and source-ordered compile-time value arguments plus
their declarations. `Index` carries exactly one nominal call or a checked
`MirIntrinsicSubscript` discriminator for Tuple/runtime-pack storage, variadic
storage, SIMD, pointers, or the CTFE-only compile-time-list bridge. `Slice`
similarly carries exactly one nominal call or the temporary VM String intrinsic.
`MultiIndex` and `MultiSet` always require a nominal call contract. Checked
assignment, including the single-index spelling
`value[index] = replacement`, retains whether the right-hand side binds as the
last positional argument or the keyword-only `value` after a variadic index
pack. A mutable receiver place may be rooted in a `mut`/`ref` parameter, so the
VM commits setter write-back through the same caller handle instead of
flattening the access into raw backing storage.

One narrow representation bridge is intentionally call-less despite its
nominal checked result: `Slice.indices()` currently produces transient private
`Value::Tuple` storage typed as the public result Tuple. Its subsequent indexing
is explicitly tagged `MirIntrinsicSubscript::TupleStorage`; this exception is
not permission to infer dispatch from an arbitrary nominal runtime value.

A projection below a nominal reference-returning accessor is rooted in a hidden
caller handle. Lowering executes the selected accessor once, materializes its
typed `ref` result and loans, then appends ordinary field/private-storage
projections to that handle. It must not recursively turn
`container[index].field` into a raw `MirPlace` rooted at the nominal container;
the extended handle is also the caller place passed to a later `mut` or `ref`
parameter or forwarded from a reference-returning function. Pointer, SIMD, and
private Tuple steps below that field retain typed dynamic/constant projections.
Verification distinguishes a place that forwards the `ref T` handle slot from
one that addresses its `T` referent, validates the `through` slot and every
concrete projected element type, and treats nominal `owner[index]` paths in
`EstablishLoans` as analytical origins rather than executable VM navigation.
Register-type closure makes the same root-sensitive distinction: borrowing an
ordinary field whose stored value is `ref T` produces an outer `ref (ref T)`
handle, while a projection through an existing `ref` or origin-bearing Pointer
forwards that capability and can never recover stronger mutability.
Assignment through a runtime alias likewise preserves the local slot's checked
`ref` type: the right-hand side types the referent write, never the handle
storage. This matters for union-origin free-function results, whose one handle
may designate any of several caller places.

Reference-valued List elements add one deliberate handle layer: indexing the
List first produces a handle to the element slot, whose stored value is itself a
reference handle. Lowering peels that outer slot handle before an augmented
write or chained method receiver is formed. The operation therefore reaches the
ultimate referent and never replaces the reference stored in the List element.
Relatedly, a chained subscript's loaded base register may legitimately stay
reference-typed one level above its place — the VM's `LoadPlace` second
dereference resolves it at runtime — so the verifier peels exactly one
`TyKind::Ref` level on both the storage and the loaded register when checking a
subscript receiver, symmetric by design. No consumer needs the register
retyped (analysis and drops read place types, not the loaded base register),
so this one-level tolerance is the sanctioned contract rather than a gap.

A place that reaches a stored handle *below* its root — through a `ref`-typed
field, or a single-pointee pointer field (`p.src[].v`) — is likewise not raw
frame storage, because the handle designates storage the executing frame may
not own. Reads, writes and drops of such a place all route through the
reference walk, which chases stored handles mid-projection and re-roots at the
storage they designate; frame-place navigation owns only handle-free storage,
plus a final dynamic index, which is a heap allocation's element or a nominal
`__setitem__` receiver.

Augmented nominal subscripts cross the checked boundary with call-local
adaptation and invalidation snapshots. For a value result this includes both
complete `CheckedCallContract`s plus the computed-result setter slot. MIR
evaluates the receiver and raw subscript operands once, evaluates the RHS,
applies getter-specific conversions and calls the getter, performs the operator,
then independently reloads any getter-mutated caller place and applies the
setter conversions before `MultiSet`. A mutable-reference getter instead is the
complete operation: MIR invokes it before the RHS, reads and updates its handle,
and emits `WriteRef` without selecting or calling a setter. The getter uses
`Index`/`Slice`/`MultiIndex` in either path, so neither source expressions nor
overloads are reconstructed.

Checked membership on a nominal struct likewise lowers to the selected
`__contains__` `MethodCall`, with a retained container place and the source value
as its argument. It does not use the value-only `BinOp` form: doing so would lose
the borrow and let a short-lived shallow receiver appear to own pointer-backed
fields. `not in` negates that checked call's Boolean result.

Intrinsic `print`, `String`, and `repr` formatting retain a nominal argument's
place, when one exists, through its `Writable` call. This is a liveness fact,
not a runtime ownership guess: drop elaboration cannot release the original
pointer-backed owner after loading the value but before formatting finishes.

`UseVar` is tagged with a `UseMode`:

```rust
pub enum UseMode {
    Copy,
    Move,
    BorrowShared,
    BorrowMut,
    Forward,
}
```

This lets later analysis distinguish ordinary reads, ownership transfers, and
borrows without reparsing expressions. `Move` and `Forward` both end the slot
(`UseMode::moves`), and ownership treats them alike. `Move` initializes new
storage, so it runs the type's move initializer. `Forward` hands the slot's
storage to an owned (`var`/`deinit`) parameter of the call that consumes the
register, so no move initializer runs, as Mojo's owned argument convention
binds the caller's storage. MIR emits `Forward` for a whole-slot `x^` the
checker marked `SemanticAdjustment::ForwardedArgument`: an argument, a
receiver, or an operator operand bound to a regular owned parameter
(`checker/places.rs`, `forwarded_arguments`), and an element an owning
(`var *xs`) collector gathers (`Gathered`, in `record_argument_borrows`), so
the pack holds the caller's storage as upstream's owned `VariadicListMem`
does. MIR also emits `Forward` for a conditional expression's merge read
(`Flatten::ternary`): each arm initializes the result temporary (a `^` arm
by a move, a place arm by a copy the checker marks
`copy_place_value_use`), and the consumer takes it as any other rvalue.

### Partial Moves

Whole-variable moves use:

```rust
UseVar { mode: UseMode::Move | UseMode::Forward, ... }
```

Field moves and constant-index moves from compiler-private Tuple storage use:

```rust
MovePlace { place, ... }
```

This allows the ownership analysis to understand:

```mojo
var x = p.a^
print(p.b)
```

as valid when `a` and `b` are distinct fields, while rejecting a later read of
`p.a` or a whole-value move of `p` before `p.a` is reinitialized.
The same rule lets generated heterogeneous-pack code move `storage[0]` and
`storage[1]` independently; runtime-indexed places remain conservatively
overlapping.

### Calls

MIR calls keep the information the VM needs for Mojo-style conventions:

- positional argument registers
- keyword argument registers
- simple caller places aligned with both positional and keyword `mut`/`ref`
  arguments
- an optional callable-value place for a nominal `mut`/`ref __call__` receiver
- source-ordered compile-time parameter arguments, retaining an optional name
  separately from the optional value register (type arguments have no register)
- the resolved lowered callee name when the checker selected an overload
- the checker-selected concrete error type when the call may raise

Non-capturing functions also have a runtime `Value::Function` representation.
Calls through a function-typed local or a general callable expression lower to
`CallIndirect`, whose callee register is resolved to a MIR function symbol by the
VM before it pushes the ordinary explicit frame. It retains positional and
keyword argument places and, for nominal callable values, the callee place plus
the exact checker-selected `__call__` symbol. A `def(...)`-typed parameter
retains the same signature as an abstract dispatch symbol; its value is a
function or closure, never a struct, since the checker rejects a nominal
callable where a `def(...)` value is expected, as the pin does. The elaborator
devirtualizes every nominal-callee `CallIndirect` into a `MethodCall` naming
the `__call__` instance, and that call is what demands the member (R1009).
Consequently same-arity `__call__` overloads do not fall back to runtime value
tags or source-name/arity selection. The checker has already matched the
arguments against the callable signature, so indirect execution shares the same
function/reference ABI without adding dynamic overload ranking.

For a generic indirect call, `CallIndirect` also carries the checked anonymous
callable's `ParamDecl` list. The VM first binds source-ordered positional and
named bracket arguments to that contract, then evaluates omitted scalar and
symbolic callable defaults in declaration order. Only after the contract has
produced concrete value slots are they renamed positionally to the
alpha-equivalent implementation binders. Contract defaults therefore govern
partial and named calls even when the runtime function value declares different
defaults. Origin/OriginSet arguments remain semantic-only and never enter this
runtime vector. When checking has concretely instantiated a generic callable,
MIR additionally retains its complete declaration-order `TyArg` list (including
defaults) and the resulting monomorphic contract. This pair is semantic-only:
the verifier reconstructs type/value substitution from it rather than searching
earlier `Const` instructions. A dependent indexed type must thereby resolve to
one concrete operand/result type before executable argument verification. A
call which remains inside an unspecialized generic body may retain a symbolic
dependent type only when every referenced compile-time name is owned by an
explicit enclosing value binder. The MIR verifier also checks the retained
declaration list against the callee register type, validates every value
register, and rejects malformed raising and statically decidable
reference-result metadata.

`TyKind::Func` and `TyKind::GenericFunc` hold their signature behind one box, a
`FuncTy` (the generic form's `GenericFuncTy` adds its parameter declarations to
one), so a `Ty` is sized for its common variants; a compile-time assertion
beside `Ty` pins that size. `Ty::callable` reads either form's signature.
The signature retains both the `raises` effect and its optional
error type. A source function type also retains a `var **kwargs: T` parameter as
the checked `kw_variadic` collector rather than flattening it into the ordinary
parameter vector, so indirect calls preserve structural keyword binding.
`SignatureKey` likewise stores a distinct keyword-variadic type key; ordinary,
positional-variadic, and keyword-variadic slots with the same element type
therefore cannot collide in lowered callable identity. A free function's
signature walks its parameters in declaration order on both the definition and
the call-resolution side, and the names of the parameters after a `*args`
collector — keyword-only by position — join the key, so `f(a: Int, *rest: Int)`
and `f(*rest: Int, a: Int)` are two declarations with two symbols. Direct
overload resolution carries the selected candidate's effect
alongside its lowered symbol, and an indirect call reads the effect from its
callable type. Generic substitution includes error types, with a nonraising
callable inferring `Never`. `Never` is the bottom type, and `raises Never` is
treated as nonraising. A protected `try` records the errors its body can raise
and gives the `except` binding that type. Effect checking therefore happens
after candidate selection instead of conservatively attaching one effect to
every declaration sharing a source name, and before lowering. The same path is
used for trait requirements and bounded method calls. `Call`, `CallIndirect`,
`MethodCall`, `Index`, `Slice`, `MultiIndex`, and `MultiSet` retain the selected
optional error type for MIR verification; the VM does not rediscover an effect
from a source declaration.

When a `TyKind::GenericFunc` appears where a concrete `TyKind::Func` is expected, the
checker solves its type parameters from the expected parameter and result types
and validates the resulting monomorphic signature. When both sides are generic,
it alpha-normalizes their binder names, preserves arity and bounds, and compares
the resulting callable shapes directionally. Binder defaults and the infer-only
marker govern calls through the contract but are not part of current Mojo's
generic-callable conformance identity.

There are two source forms that otherwise look similar in a declaration's
compile-time parameter list:

- `F: def(T) -> T` declares a type parameter with a checked anonymous callable
  constraint. Its `ParamDecl::Type { callable_bound, ... }` retains the complete
  dependent `TyKind::Func` or `TyKind::GenericFunc`, so a call through `F` inside the
  template is checked against that signature. A monomorphic contract requires
  one monomorphic function type or a struct with nominal `def(...)` conformance;
  a contract with its own `def[...]` binders accepts an alpha-equivalent generic
  function. A shape-compatible `__call__` and an unresolved overload set do not
  satisfy either form. A nonraising/read-only implementation may satisfy a
  raising/mutable contract because it demands no more capability, while binder
  bounds, ownership-changing conventions, and reference origins remain exact.
- `callback: def(...) thin` and `callback: def(...) capturing[...]` declare
  callable **value** parameters. `Origin` and `OriginSet` binders are
  semantic-only, and `//` separates infer-only binders from explicitly supplied
  ones. The bracket argument is evaluated to a callable register, then reified
  under the parameter name as a hidden typed MIR local; it is not counted among
  the function's ordinary source-call ABI parameters. Calls through that local
  use `CallIndirect` like any other callable value. The callable type may have
  its own nested `def[...]` binder scope; ordinary runtime parameters cannot use
  that parametric type directly and must use one of these compile-time forms.

An omitted callable-value argument is represented by `CallableDefault`, not by
`CtValue`. `Symbol` retains the checker-selected function specialization,
`Parameter` aliases an earlier reified callable, and `If` selects recursively
between those plans using a dependent compile-time condition. The VM evaluates
the plan in declaration order after preceding scalar and callable parameters
have been reified. The generic identity carries only a symbolic occupied-slot
marker, so neither a static function nor a captured environment is serialized
as a compile-time closure payload.

An explicit `function[origin_of(place)]` expression is the current-Mojo spelling
for materializing an origin-generic function as a value without erasing its loan
contract. The checker binds the function's semantic-only Origin parameters to
the caller's checked origins and produces a concrete `TyKind::Func`; its parameter
reference signatures and reference-result signature are then ordinary indirect-
call facts. The runtime value still needs only the resolved MIR symbol because
the origin substitution has already constrained legal calls and escapes. A
non-overloaded stateless nested function combines the same substitution with its
resolved lifted symbol, so top-level and lexical function values share indirect
lowering. Origin arguments are split from ordinary compile-time arguments before
candidate checking. They therefore participate in direct overload selection,
compose with inferred generic arguments on direct calls, and compose with
explicit ordinary generic arguments on function values. Specialization binds
the complete source parameter layout: a variadic type/value pack consumes only
the positional segment available before required suffix parameters, while a
named Origin may follow the pack and remains on the rewritten call. Origins,
OriginSets, and callable-value parameters stay symbolic on generated
specializations instead of shifting the evaluated type/value arguments; a
checked type's origin tail spells as its binder where the clone has one in
scope and as upstream's `_` placeholder otherwise. An
overloaded function value requires an expected `def(...)` type; the checker
specializes every candidate, retains the unique compatible lowered symbol, and rejects an
uncontextualized or still-ambiguous set. Mojito can also combine a captured
nested environment with an explicit-Origin contract; the pinned nightly rejects
materializing that form, so it is classified as an acceptance divergence.

Overloaded function values remain overload sets until an expected `def(...)`
type selects one candidate; the selected lowered symbol and its raising effect
are retained in checked data. A struct is callable only when its declaration
nominally lists a `def(...)` conformance and its `__call__` method matches that
contract. Function-type generic parameters, `mut`/`ref` conventions, reference
origins/results, and raising effects are retained while checking that contract.
This applies both to parameters declared inside `def[...]` and to a dependent
outer `F: def(...)` bound. Indirect VM calls dispatch such values to the checked
method—including a signature-qualified same-arity overload—and carry the
receiver place when `__call__` needs writable/reference access. Typed-MIR
verification checks an abstract target against the callable type, then checks a
concrete nominal target against its declaration and argument types whenever the
receiver is statically known.

The environment is semantic callable-type data. `CallableEnvironment`
distinguishes an unqualified contract, an explicitly `thin` function, and a
`capturing` contract. A capture set is one of `Infer` (`capturing[_]`), a stable
declaration-order `OriginSet` parameter, or a canonical concrete list of
`CaptureOrigin { origin, access }` entries. The latter records read versus write
access, flattens origin unions, removes duplicates, and lets a write subsume a
read of the same origin. An `OriginSet` is deliberately not a reference-origin
union: it describes storage retained and accessed by a callable environment,
not storage a returned reference may designate. Substitution and nominal
callable conformance preserve the environment contract, and origin escape
checking recursively visits concrete environment dependencies.

Escape enforcement is a checker origin rule at *both* frame boundaries. The
return boundary checks a returned value's aggregate origins and a `ref[o]`
contract's actual origin; the store boundary applies the same predicate at
place writes whose destination root outlives the frame (a parameter or
`self` owner from the per-body escape context, including variadic collector
parameters): a store fires only when the stored value carries loans — or the
destination is `ref`-typed storage, whose rebound handle becomes the loan —
and some origin is rooted at frame-local storage. Parameter-rooted loans
store outward freely, so origin-parameter-bound flows such as
`self.field = make_iter(self.data)` stay accepted. Ownership analysis remains
the lazy within-frame layer (interior origins are invalidation generations,
not exclusive loans; eager whole-loan exclusivity is not promised). Mapping
key yields are declaration-level immutable via the `ref` signature's
immutable-origin cast (`ImmOrigin(...)`), which
`lower_ref_sig` unwraps and pins to `SigMutability::Immutable` so the loop
site's parametric-mutability upgrade never applies; the upgrade direction is
rejected.

Nested functions are lifted with explicit closure environments. Capture lists
appear directly as `{...}` after effects; the removed `unified {...}` spelling
is rejected with a contextual parse error. `imm`, `mut`,
and `ref` captures become stable frame/slot handles with their checked
permissions, `var` clones the value when the declaration executes, and move
capture transfers the value at that same point. Direct nested calls use the same
materialized closure and `CallIndirect` path as closure values rather than
rebuilding an environment at each call.

The stored reference handle is not itself a persistent loan spanning declaration
to invocation. The enclosing code may access or update its owner between those
events, and an `imm` capture observes that current value. Loan conflicts apply
when the closure is used under its checked capture convention; owned copy/move
captures instead operate on their independent environment slot.

The checker resolves every explicit, defaulted, and intermediate forwarding
capture to a stable `OwnerId`, exact storage `Ty`, and capture convention. This
includes explicit captures unused by the body: copy/move still happens when the
declaration executes. Checked call, identifier, type-application, and declaration
occurrence identities then select a nested registry keyed by `OwnerId`; same-name
declarations in disjoint blocks cannot alias. Lifted capture parameters and MIR
declarations use the retained storage type rather than an opaque environment
placeholder.

Each checked capture also carries the origins retained by its stored value and,
for `imm`/`ref`/`mut`, the read or write access to its source place. Finalizing a
nested declaration folds those facts into its concrete callable environment.
At every direct, indirect, method, constructor, or nominal subscript call, the
checker collects the concrete environment accesses of both the callee and
non-escaping callable arguments into
`SemanticAdjustment::CallableCaptureAccesses`. Checked HIR keeps that
adjustment; MIR maps stable owners to local places and stores the result on the
call instruction as `capture_accesses`. Verification checks the typed call,
and persistent-loan analysis treats each entry as an access only for the call's
duration. This is why a mutable closure invocation conflicts with a live alias
of its captured owner even across a downward-funarg boundary, while ordinary
owner mutation between closure declaration and invocation remains valid.

Capture discovery builds a lexical tree and lifts descendants recursively at
arbitrary depth. Each lifted symbol encodes its complete lexical path; the
nearest declaration wins under shadowing, and each intermediate environment
forwards only captures permitted by its own explicit/default policy. Capturing a
sibling captures the already materialized sibling closure slot, not a rebuilt or
flattened copy of its transitive environment, so declaration-time snapshots and
moved state remain stable. `KeepAlive` retains exact existing callable/reference
slots and never fabricates a slot for a transitive capture; it extends a live
range for the lifetime check and is erased by `lower-lit` after drop
elaboration. Lifted
specializations retain their exact regular/default/variadic/keyword/marker/effect
ABI; reference returns and a named `out` result remain part of that checked ABI.
The checker rejects undeclared captures and every path that would let an
environment outlive its defining function.

Compile-time specialization distinguishes evaluated parameters from retained
runtime/semantic parameters. Scalar values, ordinary types, and packs may drive
`comptime if` or `comptime for` and are baked into the generated body. Origin,
OriginSet, and explicit callable-value parameters remain on its signature, and
their source arguments remain on the rewritten call. A scalar-controlled branch
can therefore select code that later invokes a captured callback without asking
the compile-time universe to own that callback. `CtValue` intentionally has no
function or closure variant: this residualization is not arbitrary callable
CTFE, and it does not make closures escaping. A capturing closure binds only
to a contract that states `capturing[...]`: unqualified `def(...)` value
positions reject it (matching current Mojo), while comptime callable bounds
still ground capturing values.

Runtime `for`, tuple-unpack, and `except` targets likewise retain checked owner
identity and storage type across HIR and structured-region lowering. Unpack uses
each target expression's occurrence/owner facts; loop and handler declarations
seed exact owner slots. A handler may therefore shadow an outer same-name value,
host a nested closure over its `Error`, and restore the differently typed outer
binding afterward without name-based slot reuse. Future pattern and coroutine
binders can extend this declaration/binder boundary instead of forging source
locations.

For method calls, MIR also records whether the receiver was a writable place.
That lets a `mut`/`ref self` method bind directly to caller storage, including
when the call executes synchronously inside a structured region.
For overloaded method calls, MIR also carries the resolved function name, such as
`Counter.bump$ov$Int`, so the VM does not have to reconstruct type-directed
dispatch dynamically.

### Overloaded Names In MIR

Overloaded definitions lower to stable signature-based names. The source name is
kept for non-overloaded declarations, but an overload set uses names of the form:

```text
function$ov$ParamType$OtherParamType
Struct.method$ov$ParamType
Struct.__init__$ov$ParamType
```

For example:

```mojo
def choose(x: Int) -> Int: ...
def choose(x: String) -> String: ...
```

lowers to functions named roughly:

```text
choose$ov$Int
choose$ov$String
```

This is intentionally a lowered compiler name, not source syntax. It gives MIR
and the VM a stable identity for each candidate, including same-arity overloads.
It also keeps arity overloads and type overloads on one mechanism instead of
special-casing `name#arity`.

Signature identity and this name scheme are owned by one canonical module,
`crates/mojito-symbol/src/symbol.rs`: a signature is typed data (`SignatureKey`, built from either
the declared `ast::SourceType` or the checker-resolved `Ty` — both spell a type
from its annotation, e.g. `Point`, `Pair$Int`) and only the module formats the
final symbol. Checker, MIR, and VM all route through it, so the recorded
callee always names the emitted function; `tests/symbol_test.rs` pins the
spellings and scans the facade's `src/` and every `crates/*/src/` for stray
hand-built `$ov$` strings.

The signature stores a keyword-variadic collector separately from positional
types and emits a `$kwv$Type` component when present. Thus
`def route(value: Int)` and `def route(var **options: Int)` have distinct ABI
identities even though their sole declared element type is `Int`; definition-
and resolution-side symbol construction use the same field.

Across phase boundaries, Mojito names source syntax and checked semantics
explicitly. `SourceType` is parser-owned syntax; `Ty`, `binding_ty`, and MIR
`param_types` are checker-resolved facts. `AnnotationSite` keys declaration
annotations at the checked boundary, after which HIR, MIR, and the VM carry only
the resolved `Ty`. In particular, variable initialization and function-entry
coercion no longer reinterpret source annotations in the backend. Missing checked
facts are compiler invariant diagnostics carried by the lowering/backend
result—not permission for compatibility APIs to guess from unchecked input.

### Try In MIR

`MirInstr::Try` contains mini-CFGs:

```rust
Try {
    body: Vec<MirBlock>,
    handler: Option<(Option<VarId>, Vec<MirBlock>)>,
    orelse: Option<Vec<MirBlock>>,
    finalbody: Option<Vec<MirBlock>>,
    cleanup: Vec<CleanupDrop>,
}
```

Those mini-CFGs share the enclosing function's register and variable spaces. They
have local block numbers, but their instructions address the same `regs` and
`vars` vectors as the outer function frame.

The structure mirrors source-level exception semantics:

- body runs first
- `except` handles a raised error
- `else` runs only when the body completes normally
- `finally` runs on every path
- `return`, `break`, and `continue` crossing the region are represented as
  non-normal flows so `finally` can run before control leaves

### Structured Region Ops

`Try` is one of six region ops. The other five are upstream's `hlcf` ops
(`docs/structured-control-flow-plan.md`): `If { results, cond, then, elifs,
orelse }` (`hlcf.if`, each `MirElif` a condition region and an arm), `Loop {
label, body }` (`hlcf.loop`), `ComptimeIf { results, conds, arms, orelse }`
(`hlcf.comptime.if`), `ComptimeFor { binder, source, body, orelse }`
(`hlcf.comptime.for`, its `orelse` one `Unreachable` block), and `Match {
cases, orelse }` (`hlcf.match`: each case a region that tests its pattern
and runs its body, left by `MatchNext` for the next case — the `orelse`
after the last — or by `MatchComplete` for the op's end; `orelse` ends in
`Yield` with no values). Their regions are block lists like a `try` part's,
and end in the region terminators `Yield`, `ElifCond`, `Break`/`Continue`
(an optional `LoopLabel` naming the nearest enclosing loop carrying it, a
`match` case between them being no loop), `ComptimeYield`, `ComptimeBreak`/
`ComptimeContinue`, `MatchNext`/`MatchComplete`, and `Unreachable`;
`Return` may end any region.
`instruction_regions`/`instruction_regions_mut` walk every op's regions in
a fixed order, and `prune_unreachable_blocks`, `block_successors`, the
analyses, and the elaborator's slot renumbering walk through them.
`mir::visit` walks every type and compile-time parameter site of a body,
regions and terminators included, as MLIR's `AttrTypeReplacer` does, and
the elaborator substitutes an instance's bindings through it.

MIR lowering builds `and`/`or`, conditional expressions, comparison
chains, and comprehensions as `If`/`Loop`, each region one block (a
conditional expression whose condition the checker found a parameter value,
`SemanticAdjustment::ComptimeConditional`, is a `ComptimeIf` with results,
as upstream's `IfElseOpNode` emits `hlcf.comptime.if`), and
`lower_cfg_nested` ends every body with `mir::lower_control_flow` (upstream's
`LowerControlFlow`) lowers them to blocks and the CFG terminators: a
region's blocks are appended to its list, the rest of the block moves to a
join, an op's results go through one slot each, stored at a `yield` and
forwarded at the join, a `break`/`continue` leaving a `try` part to a
function-level loop is an `EscapeJump`, a `match` case's `match.next` and
`match.complete` jump to the next case's entry and to the join through a
frame like a loop's (`tests/mir_passes/lower-control-flow/match_cases.mir`),
and `Unreachable` is `Return(None)`.
Every analysis below lowering, the elaborator, the VM, and the Pliron
backend sees only lowered MIR.

### Spans

MIR records source spans for generated registers:

```rust
pub struct SpanTable(pub HashMap<u32, (SourceSpan, Option<VarId>)>);
```

This is what lets ownership diagnostics point back to the original source even
though expressions have been flattened into temporaries.

## Stage 6: Ownership Analysis

Module:

```rust
crates/mojito-analysis/src/analysis.rs
```

Production entry point:

```rust
check_ownership_checked(program: &CheckedProgram) -> Result<(), OwnershipError>
```

`check_ownership(&[Stmt])` is a compatibility wrapper for unchecked callers; it
rechecks through compatibility lowering. Production compilation lowers the
existing `CheckedProgram` and runs move/init and persistent-loan analysis on each
function.

The core state is:

```text
Owned
Moved
MaybeMoved
```

Analysis is forward over the MIR CFG.

Rules:

- defining a variable makes it `Owned`
- moving a variable makes it `Moved`
- using a `Moved` variable is a use-after-move error
- merging `Owned` and `Moved` at a join produces `MaybeMoved`
- using a `MaybeMoved` variable is a conditional-move error
- moving a field marks that field moved; a later use of any disjoint part of
  the value is an error until the field is written back
- a whole redefinition of the variable, or the function's exit, while a field
  is moved out is an error (the value cannot be destroyed as a whole)
- a `deinit` parameter's direct fields are independent values: each may be
  moved out and is destroyed at its own last use
- reassigning a moved variable or moved field reinitializes it

This is why control-flow lowering happens before ownership analysis. A move
inside an `if` or loop only has the right meaning once joins and back-edges are
explicit.

A compile-time region follows the same rules. A `comptime if` is decided as an
`if` whose condition is opaque, and a `comptime for` as a loop whose trip
count is unknown: an arm assumes nothing from its condition, the join
intersects the arms, and a value consumed in a loop body without a refill is
consumed on the back edge. Substitution keeps the taken arm with the drops the
analysis placed and never recomputes a last use
([`docs/notes/comptime-region-ownership.md`](notes/comptime-region-ownership.md)).
A `comptime if` in a generic `def` reaches the analysis as that region: the
`ComptimeBranch` terminator joins like a `Branch`, and the elaborator keeps the
taken arm with the destroys the analysis placed at its entry. A `comptime
for` the template serves reaches it as a loop: the `ComptimeFor` header's
successors are its body and its exit, the body's back edge returns to the
header, a compile-time `break` and `continue` are the loop's edges, and the
elaborator's copies carry the destroys the analysis placed in the body.

### Persistent local loans

Local `ref name = place` bindings are checked references, not copied referent
values. MIR emits one grouped `EstablishLoans` operation for each fresh binding
generation; every `MirLoan` retains the executable owner `MirPlace`, permission,
optional canonical `MirInteriorOrigin`, and whether it is a shared alias (a
`Pointer(to=place)` loan: two shared loans of overlapping storage coexist,
while owner accesses and exclusive loans treat each as an ordinary borrow), and
whether it is a setup loan. A setup loan holds a by-value call operand's
reference result in its hidden `$call_ref_r` slot until the call's setup
reads it: it keeps its owners live for drop elaboration and carries its
interior generation, but excludes no access and no other loan, since Mojo
judges argument exclusivity at the call itself; the move analysis reads its
owners at each read through the slot (`MoveContext::setup_owners`), so a
later argument's move makes the read a use of the moved value.
Statically resolvable aliases use the frozen place, while cross-call aliases
use explicit reference operations.
`MirPlace::through` records which reference authorized an access. The VM erases
both loan and interior-generation metadata after checking.

Reference variables participate in backward CFG liveness. A loan is active from
its binding through its last use, including joins and loop back-edges. While it
is active, direct or differently-authorized overlapping mutation, replacement,
move, drop, or mutating call is rejected. Establishing an interior-generation
loan (a `for ref` loop's element loan) beside a whole-place loan of the same
storage, or the reverse, conflicts when either side is mutable; two interior
loans stay generation-tracked rather than exclusive. Projection overlap is
field-sensitive and index-conservative. Drop elaboration combines that
liveness with the
forward reaching `EstablishLoans` generation: every possible owner of a live
union/reference value remains alive, but rebinding a reference-bearing aggregate
retires only its old generation instead of permanently retaining every owner
ever stored in that variable slot.

Reference handles can temporarily leave a variable slot while one MIR
expression is evaluated. Drop elaboration therefore propagates owner provenance
through the SSA registers of that expression (`MakeRef` through `ReadRef` and
the consuming call/operator). The owner dies after the final consuming
instruction, while `DefVar` is an explicit handoff to a newly established
variable generation. This preserves ASAP destruction without dropping storage
between creation and use of a transient handle.

Cross-call aliases lower to explicit `MakeRef`, `ReadRef`, and `WriteRef`
operations. Runtime handles contain a monotonic frame identity, variable slot,
and captured field/index projection. Returning a reference forwards the caller's
handle, so a union return preserves whichever argument was selected dynamically.
An ordinary value context requires a `Copyable` referent and follows `ReadRef`
with typed `CopyValue`, invoking the referent's copy lifecycle instead of
aliasing pointer-backed storage. An explicit `ref` binding retains the handle,
omits that copy, and therefore remains legal for a linear referent.
The checker uses the same explicit `CopyValue` boundary for a projected place in
a consuming context such as a new binding, assignment, or return. This fact is
recorded while conditional `Copyable` constraints are in scope; plain
`LoadPlace` remains handle-preserving for borrowed method receivers, formatting,
and iteration.
The checker marks only actual arguments selected for `mut`/`ref` parameters as
caller-place dependencies; MIR retains those places through the call while an
ordinary copied place argument remains eligible for ASAP destruction after its
value is evaluated.

A bare `ref` parameter or receiver has parametric mutability. Its checked body
may read and return/reborrow it, and the call substitutes the actual caller
capability into a reference result, but the body cannot assume write access.
Writing requires an explicit `ref[origin]` whose `Origin` is mutable (or the
ordinary `mut` convention). Call solving rejects any attempt to pass an
immutable reference to that mutable contract, so capability cannot be escalated.

### Cross-call transfer effects

A callee that stores a loan-carrying value into `self` or a `mut`/`ref`
parameter changes the caller's loan picture, so the checker owns that fact and
replays it at every call. When the store-outward escape rule accepts such a
store, the body records a `TransferEffect { dest, src, src_is_place, mutable }`
in signature-origin terms (`Self_`/`Param(k)`) on the current callable's frame;
the bundled `List.append`/`insert`/`__setitem__` are seeded directly because
their pointer-mediated stores never reach that acceptance point. Effects live
in a callable-identity-keyed side map: bare names for unique callables and
signature-qualified symbols for overloaded methods. This keeps a consuming
candidate from inheriting a borrowing sibling's loan effects (and vice versa).
Visibility is
declaration-order independent: `check_program` reruns the whole check — a
fresh checker seeded with the prior round's committed map merged over the
bundled seeds — until no call site has observed a stale callee entry.
Staleness is exact rather than structural: `apply_transfer_effects` records
the first-seen effects per queried callee (including "none"), and a round
converges when every observation matches the final committed map, so a
program whose effects were all committed before any call site consulted them
— every stdlib-only compile included — finishes in one round. Effects grow
monotonically over a finite per-callable lattice; a four-round cap surfaces
`TransferEffectDivergence`, which indicates a checker defect, not a user
error.

The store-outward acceptance point itself is shared: the SetPlace guard
(escape check plus transfer recording) is the `check_outward_store` helper,
which unpack-into-place targets also run (per tuple-display element, or
conservatively with the whole right-hand side's origins). An outward store of
a symbolic value that may carry a loan once instantiated records its effect
as latent: the frame keeps it, in order, beside the published effects
(`TransferFrame::record`) and never publishes it; the elaborator's instance
of the body carries the loan where the substituted type does. The same store
publishes one effect whose source is the stored type itself
(`SigOrigin::Carried`), as does a symbolic value handed by value to a callee
that stores it. A call closes that type with the arguments of the struct
instance its receiver is and with the types it binds to the callee's own
binders, and the destination borrows, shared, the places the closed type
names. That is what serves an instance over a loan-carrying argument, and a
generic `def` called at one, from its template with no clone
(`docs/notes/generator-contract.md` §Carried sources). A nested `def`
records no such effect. A bundled seed stays in a method's summary beside
what its body records. Outward storage
covers both the frame's outliving owners (`self`, `mut`/`ref` parameters and
the capture-reachable extensions of the escape context) and ANY
enclosing-frame binding a nested def reaches through captures — storing a
closure-locally rooted loan into a captured enclosing local dangles just as
surely once the closure returns. A store through a captured owner records a
concrete `SigOrigin::Bound` destination (owner ids are checker-global):
invocation sites ground it directly, intermediate frames propagate it
verbatim, and the frame whose signature covers the owner re-abstracts it, so
a method whose closure stores into captured `self` carries the effect to the
method's own callers. Augmented assignment needs no dedicated guard: the
in-place dunder rides ordinary method selection, so its callee effects
replay at the `+=` site.

Effects also ride checked function types. A `def` name in value position
bakes its committed effects into the produced `TyKind::Func`/`TyKind::GenericFunc`
as an identity-transparent `TransferSet` (never part of type equality or
acceptance — a `def(...)` contract cannot spell effects, so soundness comes
from call-site replay, not acceptance filtering), with a fixpoint
observation so a later-grown entry re-bakes. Indirect calls replay from the
value's type; a callable-struct call replays its `Struct.__call__` entry
with the callee binding as the receiver; overloaded method calls replay the
selected signature-qualified entry (free-function overloads retain their
shared bare-name entry); and a trait-method call on a bounded receiver —
which has no concrete body — replays the union of effects over every
conforming implementation and overload of the method, one observation per
conformer key.
The one genuinely higher-order shape is a body calling through its own
callable parameter (a runtime `def(...)` param or a compile-time callable
value param, which specialization retains symbolically): the body records a
`CallThroughEffect` carrying the signature abstraction of every actual, and
each call site — which knows the concrete callable — translates that
callable's effects through the recorded mapping into effects of the callee
and replays them, rejecting a frame-local source flowing into a signature
destination; when the supplied callable is itself a callable parameter of
the calling frame, a composed residue is derived instead, so two-level
forwarding chains resolve at the outermost concrete call. Call-through
visibility shares the two-phase pass through its own seed and observation
channel.

Each call site with a matching effect substitutes the source actual's caller
origins (its carried aggregate/reference origins, plus — only when
`src_is_place`, i.e. the callee parameter is borrowed rather than owned — the
actual's own place), enforces the store-outward escape rule across the boundary,
merges the result into the destination actual's aggregate-origin bookkeeping so
the checker's own return-escape analysis sees callee-installed loans, derives a
transitive effect onto the enclosing callable when the destination roots at its
parameter or receiver, and records a span-keyed
`CheckedCallTransfer { dest, dest_path, sources, mutable }` on
`CheckedProgram` for MIR. Destinations are interior-precise: the store's
path below the destination root survives on the effect
(`SigOrigin::Projected`), composes with the actual's own projection at each
call site, and lowers as the generation's destination domain —
`EstablishLoans { reference, loans, marker, dest_interior }`, where `None`
means the whole root. A root-domain generation replaces every prior
generation; an interior-domain generation replaces only overlapping interior
domains, so sibling fields keep independent generations, and a `Store`
through a concrete field prefix releases the domains it covers — rebinding
`t.a` frees `t.a`-rooted transferred loans while `t.b`'s and the root's
survive. Repeated transfers into one domain still merge (union, never
replacement), so a second `append` extends that generation. Lowering skips
destinations rooted at the current function's own parameters, which the
derived effect covers at the caller where the storage lives; a `Bound`
destination resolves through the owner-variable map only in the frame that
owns the storage. Ownership and drop analysis then reject mutating or
dropping the loan root while the stored alias lives and keep borrowed
sources alive under carrier collections with no transfer-specific analysis
code. A closure value flowing into storage additionally loans its REFERENCE
captures' owners (`imm` immutably, `mut`/`ref` mutably) — the stored
environment retains their frame slots — while direct nested calls keep the
loan-free declaration-to-call capture model. The `FieldInvocation`
adjustment (an indirect call whose callee place is the storage's, so a
closure environment rehydrates from stable storage) is retained internal
machinery: `def(...)`-typed fields and elements are rejected at declaration
to match current Mojo, so no production path reaches it from a field today,
and it stays only as the invocation shape for supported callable storage
channels.

The deliberate residues, frozen with the schema: effects erase when an
effectful callable value is stored into explicitly annotated `def(...)`
storage (a plain function value carries no loans of its own, so this stays
permissive rather than unsound); a call through a bare `callable_bound` with
no value provenance carries nothing; a call-through destination that is
frame-local to the higher-order body is invisible to that body's own
return-escape analysis; transfer via a returned `self` belongs to the
return-origin path; and source loans stay root-abstracted — only
destinations carry interior paths.

### Nominal String and the literal bridges

The self-hosted `String` (stdlib/std/string.mojo) is an ordinary struct — a
UTF-8 byte buffer over `UnsafePointer[Byte]` in the List storage pattern —
and its literal constructors are ordinary Mojo bodies. The one intrinsic is
upstream's pair of `StringLiteral` byte primitives, `byte_length()` and
`ptr() -> Pointer[Byte, ImmStaticOrigin]` (`pop.string.size` /
`pop.string.address`): the VM addresses a never-freed allocation interned
per literal text, the native backend the interned `mjstr_<n>` constant or a
runtime descriptor's bytes. `String(literal)` allocates and `memcpy`s from
`ptr()`; `StringSpan(literal)` (upstream's `StaticString` initializer) views
`ptr()` through `unsafe_mut_cast[True]()` and `unsafe_origin_cast`. Writing
goes the other way through upstream's `Writer.write_string(mut self, string:
StringSlice)`: `String.write_to` hands the writer `StringSpan(self)` and
`StringSpan.write_to` hands itself. The VM's builtin text accumulator and the
native print path read the view's bytes; a user writer's `write(...)` gets
each formatted argument as a view over a buffer freed after the call.
Everything else (comparison, concatenation, membership, hashing, decoding,
slicing, and the result APIs, which live on the view with `String`
forwarding through `StringSpan(self)`) is pure library code.

The `StringLiteral`-vs-`String` split is realized at the type level. The
compile-time literal type is `TyKind::StringLiteral` (spelled `StringLiteral` in
source, mirroring `TyKind::IntLiteral`); string literals and literal-only
operations stay on it, as do comptime strings, `[text: String]` value
parameters, kwargs keys, and Writer payloads inside the stdlib. A source
`String` annotation is not a builtin type keyword: it parses as an ordinary
name, the prelude rewrite qualifies it, and ordinary struct lookup resolves
the nominal struct — no dedicated checker path. A literal converts wherever
the nominal String is expected through the ordinary implicit-conversion
engine (the `@implicit` literal constructor), including operator operands
(mixed literal/nominal comparisons and concatenation normalize onto the
struct's dunders), tuple-display and fieldwise-construction elements, and
specialized-pack constructor arguments. Builtin string producers retarget:
`String(x)` stringify, `input()`, `repr(x)`, and `.format(...)` type as the
nominal String by routing the underlying call to the VM's conversion builtin
(a `ResolveCallable` adjustment where needed) and wrapping the buffered
result through the literal constructor. `Error(msg)`/`raise` and the Writer
`write_string` contract accept either spelling; the VM bridges read a
nominal message back and materialize a nominal payload for a
nominally-declared `write_string`. Overload symbols spell the two types
distinctly (`String` and `StringLiteral`), so `def f(x: StringLiteral)`
beside `def f(x: String)` is a legal overload set and a literal argument
selects the exact `StringLiteral` member (exact matches rank above
conversions), as upstream. In unlinked
seam programs (no prelude) a bare `String` annotation fails explicitly as an
unknown type, and the bare `String(...)` builtin keeps the literal result.

Keyword subscripts are the general feature the String's explicit index forms
ride on: a named bracket argument over a lowercase (value) base parses as a
`MultiIndex` with `SubscriptArg::Keyword`, selected against keyword-only
`__getitem__` parameters through ordinary structural call binding and carried
to the VM through the subscript instruction's keyword channel; a named
bracket over a capitalized type name stays compile-time parameter
application. Keyword-only parameter names are part of callable identity —
`same_method_shape` compares them at declaration and `SignatureKey` appends a
`$kw$...` suffix (only when keyword-only names exist) in both symbol
producers, the checker's call-target mangling and MIR's declaration-side
mangling.

### Collection-owned interior origins

An origin path may contain `Interior("tag")`, distinct from an unknown runtime
index. It names storage owned behind a container, such as
`values["element"]`, `mapping["value"]`, or a Variant payload. Each
`EstablishLoans` marker is a fresh generation and may retain several possible
origins for a union-valued return. Multiple overlapping interior references may
coexist, ordinary owner reads remain legal, and a direct List element write
updates the storage those references designate.

The checker, rather than MIR syntax inspection, records every operation that may
redefine an interior. Lowering preserves those facts as
`InvalidateInteriors { base, except, include_base_generation, marker }`
immediately before the operation. The facts serve the ownership analysis
and drop elaboration only; `lower-lit` erases them after drop elaboration
(§Pre-Elaboration Pipeline).
Whole-owner replacement (including writes through references, reference-valued
aggregate fields, and origin-bearing pointers), replacement of an interior that
owns deeper interiors, structural List mutation, mutable/ref calls, Dict
replacement, and Variant tag replacement invalidate matching old
generations; a Dict lookup, like a List element read, does not, as the pin
keeps an earlier value reference valid across a second lookup. `except`
preserves the generation used to mutate through an interior reference while
still invalidating any nested interiors below it. Ordinary mutation leaves the
exact base generation valid; `include_base_generation` would instead
invalidate that exact named region too, and no checked fact sets it today.

A separate forward may-analysis carries generation sets through CFG joins and
loop back-edges. An invalidation on any incoming path makes a later use of that
generation an error; the diagnostic identifies the canonical origin and points
both to the stale use and the invalidating operation. Prefix matching is
field-sensitive, so mutating `pair.left` cannot invalidate
`pair.right["element"]`. Structured `try` regions carry distinct normal,
raising, and return/escape channels: handlers join only actual raising sites,
`finally` is checked on every channel, and only normal fallthrough reaches the
instruction after the region. This generation analysis is deliberately
separate from ordinary shared/exclusive loans: interior invalidation governs
storage identity, while ordinary loans continue to govern direct place access.

Origin-parametric aggregate fields store those same handles rather than reading
and copying their referents. A normal field access reads through the handle;
assignment writes through it. MIR transfers the originating loan to the
aggregate binding. Reaching-generation-aware drop liveness keeps the current
owner alive through aggregate handle use, releases the old owner when the
aggregate is rebound, and retains the replacement generation independently.
A stored `MutUnsafeAnyOrigin` reference is rejected because it would hide an untracked mutable
capability behind an otherwise safe value.

`UnsafePointer(to=place)` rides the same machinery. The checker infers
`PointerOrigin::Place` from the source place — mutability follows the owner
binding — and the checked pointer type retains that provenance through HIR and
MIR while the VM value stays an origin-free frame/slot handle. Construction
lowers to `MakeRef` plus `EstablishLoans` on the pointer binding; a stably bound
pointer's `p[0]` deref substitutes the frozen owner place (`MirPlace::through`
names the pointer), so owner liveness, ASAP destruction, and loan conflicts stay
exact, while reassigned or field-loaded pointers read and write through their
runtime handles. Aggregates that store place-origin pointers carry the owner
loan exactly like reference-valued aggregates. Because an origin-bearing
pointer designates one checked value rather than an allocation, the checker
rejects non-zero offsets, pointer arithmetic and comparison, `free()`, writes
through immutable or unresolved symbolic provenance
(`pointer_write_capability`: a pointer field's `Origin[mut=m]` binder resolves
through the holder's construction-time origins — for a call result, the
origins its callee's view-return contract (`ViewReturnOrigin`, resolved into
`call_result_origins` at the call) bound with the callee's own capability —
and the dereference place carries that capability rather than the holder
binding's; a call result holding the dereferenced pointer materializes as an
anonymous owned slot, `materialize_temporary_holder`), and returns that
would escape the origin
(`returned pointer escapes storage outside its declared origin`). A method may,
however, return the *dereference* of an origin-bearing pointer field whose origin
is a struct/callable parameter (`def get(self) -> ref[o] Int: return self.p[0]`):
the returned `ref[o]` stays within that parameter, so the return-boundary
re-rooter (`canonical_reference_parts`) follows the field handle to the single
pointee and retains the residual offset-0 index, which the runtime projection
walkers forward as the identity deref of that pointee — an immutable origin reads,
a mutable origin writes through the caller's storage. A read of the bound pointer
variable itself — a copy into another local, an annotated binding, or a call
argument — is a plain `UseVar` value read of the stored handle, which both
backends interpret as the pointee's address.

### Loops

Loops matter because a move in one iteration can affect the next iteration.

For example:

```mojo
var x = Box(1)
for i in range(3):
    var y = x^
```

The back-edge makes the moved state flow to the next iteration. The analysis can
therefore reject the second iteration's attempted move.

Borrowed iteration, and consuming iteration for a type with `__iter__(var self)`,
first execute the checker-selected nominal `__iter__` normalization chain. Every
borrowed source is retained in its own slot — statement loops and comprehension
clauses share the rule — and `GetIter` writes the normalized iterator into a
distinct iterator-object slot, so the source stays live in its own slot through
the loop rather than being overwritten during normalization. Whether the loop
keeps that source alive is the checked `IterationProtocol::source_retained`
fact: the normalized iterator may refer to its source only when its declaration
binds an `Origin`/`OriginSet` parameter, holds a reference or pointer field, or
its `__next__` returns a reference. When it cannot, nothing extends the
source's lifetime and ASAP destruction runs its `__deinit__` right after
`GetIter`, as upstream does. Otherwise a borrowed **temporary** — the only
owner of its storage — is `Bind`-bound and kept live by a `KeepAlive` liveness
anchor at the loop exit, then destroyed exactly once after the loop. A borrowed
**named** source is instead `MakeRef`-bound (a genuine reference, no copy) and
its dependency is recorded as a loan — a whole-place shared loan, or an
interior `element` generation for a concrete collection place — re-established
on the long-lived iterator-object slot: the loan keeps the source live through
the loop (no `KeepAlive` needed) and rejects conflicting mutation of the source
during iteration; the reference slot is read only by `GetIter` and dropped
afterward as a no-op. Owned iteration keeps the
single slot: `__iter__(var self)` consumes the source into the iterator.
Bundled borrowed paths cover List, Set, Dict, and Range; the bundled owned path
is currently List-specific. For a current typed-raising iterator, `TryNext`
invokes `__next__(mut self)`, writes the mutated iterator back, and branches on
whether the call returned an element or raised the exact checked
`StopIteration`; a generic `Iterable` bound takes the same path through the
abstract `__iterator_dispatch.__next__`, and a named source of a parameter
type is borrowed in place as a nominal one is, not copied into the loop. Bundled Range/list/set/dict iterators
and concrete user iterators use that nominal path. `HasNext`/`Next` serve only
the method-free fallbacks: the CTFE-only `ComptimeList` carrier and the
compiler-private heterogeneous runtime-pack carrier; public `Tuple` values are
nominal and do not use that fallback.

Borrowed concrete List, Set, and Dict place iteration retains an `element`
interior-origin loan for the live iterator. For List, nonstructural replacement
remains visible, while a structural mutation invalidates that generation and a
subsequent iterator use is rejected. Concrete List `for ref` similarly creates
write-through indexed element references. A user-defined reference-yielding
iterator can now satisfy an abstract value `__next__` contract for a `Copyable`
element and execute through the checked copy adapter. Generic code still cannot
derive a borrowed source loan, yielded-reference origin, or abstract `for ref`
binding from the associated iterator contract until the bundled protocol and
loop source/binding modes are migrated.

Consuming `for var item in collection^` moves the source once into the iterator
slot. Each `Next` transfers one element, so the current loop binding and the
residual iterator state have disjoint ownership. Normal exhaustion leaves no
residual elements; return, raise, and `break` paths run the ordinary edge
cleanup, and the exit edge drops the iterator slot. Protocol-driven owned
iteration carries current Mojo's element bounds: the yielded element must be
`Movable & Deinitable`, enforced both by the bundled `__iter__(var self)`
where clauses (an unavailable declaration rejects with the bound named) and
by a checker gate over user-declared owned iterators, so the implicitly
dropped iterator always has its residual-destroying `__deinit__` available.

Variadic packs are not library iterators, and linear whole-pack forwarding
remains supported under guaranteed exhaustion. For that channel the checker
still rejects every abandoning path when the element type is not
`Deinitable`: the syntactic body walk (break/return/raise), an observation
frame that flags any raising call whose `try` handler sits outside the loop
(callable bodies push barrier frames so nested `def`s never mark an enclosing
loop; the frame records the `handled_raise_depth` at loop entry, so a `try`
inside the body contains its error), and — in comprehensions — filter clauses
over a linear binder, since a skipped element would be abandoned. Each
diagnostic names the element's `@explicit_destroy` obligation.

### Partial Move Tree

The analysis tracks places at field granularity. This is stricter and more useful
than only tracking whole variables.

It can distinguish:

```mojo
var a = p.left^
print(p.right)   # ok
print(p.left)    # error
```

Dynamic indexed moves are more conservative because arbitrary indices can alias.
Compiler-private heterogeneous Tuple storage is the narrow exception: a
compile-time element index lowers to `Proj::ConstIndex`, so the move tree can
distinguish element 0 from element 1. This lets `Tuple(*args^)` and
`consume_elements` relocate linear pack elements exactly once while public
runtime-varying indexed transfers remain rejected.

`try` regions are walked with three channels per region (normal fall-off,
raise points, `return`/escape exits), the interior-origin analysis's shape:
the `except` arm starts from the join of the states at every potentially
raising instruction of the body (after that instruction's effects, with a
named destructor's trailing `ConsumeVar` folded in, since the receiver is
consumed at the call), `else` from the body's completion, and a `finally` is
checked from the join of all three and re-summarized per entering channel.
The states after the `try` are the join of the handler's and `else`/body's
completions, so a value consumed before a raising call is uninitialized in
the handler and after the `try`, while one consumed after the body's last
raise point is initialized in the handler. Definite and path-dependent
transfers report upstream's `use of uninitialized value 'x'`.

## Stage 7: Liveness And ASAP Destruction

Same module:

```rust
crates/mojito-analysis/src/analysis.rs
```

Entry point:

```rust
elaborate_drops_program(prog: MirProgram) -> MirProgram
```

ASAP destruction is implemented as a MIR rewrite. The analysis computes where
owned variables stop being live and conservatively splices explicit:

```rust
MirInstr::DropVar { var, destructor }
```

after each variable's last use. Its last step, `name_destructors`
(`analysis/destructor_calls.rs`), sets each `DropVar` and `DropPlace`'s
`destructor` to the `__deinit__` the dropped value's struct records in its
member table (`MirDeclarations::destructor`), as upstream's destructor
insertion leaves a call naming the `__del__` at each death, so a symbol
walk, the elaborator, and `eliminate-dead-symbols` follow the destructor as
they follow a call's callee, and neither backend looks it up from the
type. Every struct whose destruction may run code has one: a struct
declaring no `__deinit__` over droppable fields gets a synthesized
`S.__deinit__(deinit self)` with an empty body, marked `nodebug` and
`synthesized` (`mir::lower_unit` for `LoweringUnit::Destructor`), as
upstream synthesizes a `__del__` for every struct that declares none;
`refine_deinit_fields` then destroys each field at entry by its own named
`drop.place`, so a drop of the struct names the destructor and reaches its
fields' through its body, and the pre-elaboration inliner inlines it at the
drop. A drop of a type whose destruction runs no code, or of a symbolic
type, names none; the elaborator names the instance's
(`Specializer::name_lifecycle_calls`), and the verifier reports a drop that
names another symbol than its struct's `__deinit__`, or none where the
struct has one. Neither backend traces a synthesized destructor's drop or
consume. A named drop whose slot holds a whole value on every path to it
is then the call upstream leaves (`mir::lower_whole_drops`, `mir/drop_calls.rs`,
run by `name_destructors` and again by the elaborator once
`name_lifecycle_calls` has named an instance's): a forwarding `UseVar` of
the slot, which ends it and hands its storage to the callee as Mojo's
owned-argument convention does, and a `MethodCall` of the destructor on
it as the `deinit self` receiver, the shape an explicit `x^.finish()`
lowers to, which both backends run as any such call. A named `drop.place`
of an owned part (fields and tuple elements, no reference on the way)
whose slot holds that part whole on every path to it is the call too, on
the part's storage, as upstream calls `__del__` on a field's address: a
`LoadPlace` of the part, the `MethodCall` with the part as its receiver's
place, and a `ConsumePlace` ending it, the shape `x.f^.finish()` lowers
to. This covers a synthesized or written destructor's field ends, a
`deinit self`'s remaining fields, and a store's overwritten field, through
a `mut self` too. An owned, `deinit`, `mut`, or `ref` parameter enters
whole. A destructor available only under a `where` clause stays a drop
before elaboration, for the elaborator to name per instance (R302). The
test is a
definite-initialization state per slot (`Held`: whole, whole but for
missing owned parts, empty, or partial), joined over a list's blocks and
followed through a `try` as upstream's `CheckLifetimes` walks its regions:
the body entered as the op is, the handler in the states the body's raise
points leave (through the raise edge's cleanup, its error slot bound
whole), a region op raising in any state inside it, the `else` as the
body completes, the `finally` on every way out of them, and the op
completing as its `else` (or body) and handler do. A `var.store`, a
whole-slot `mark_initialized`, or a `for` loop's iterator fills the slot;
a declaration, a drop, a consume, a moving use, or an owned `iter.init`
consuming it into an iterator slot of its own empties it; a move,
drop, or consume of a field or tuple-element path marks that part missing,
a consume subsuming the parts inside it already missing, and a store into
it fills it again; reads, borrows, calls, iterator
advances, and lifetime markers keep it; compiler-private iterator storage
normalized in place leaves it partial. A `for` loop binds what
`iter.try_next` yields only past its exhaustion test, in the body, so its
`$yield` slot is never filled on the exhausted edge. A named drop of a slot
empty on every path to it is erased, since nothing dies there. Every other
named drop stays a `drop.var`, whose runtime partial-aggregate rule,
Pliron's drop and leaf flags, and an owned pack's reverse order carry
the states upstream's lifetime checker makes definite; once the
move and initialization checks walk regions (R875) the lowering reads
their state instead of its own.
A cleanup list's entries name their destructor as a `DropVar` does
(`CleanupDrop`), named by the same `name_destructors` and respelled by the
same `name_lifecycle_calls`, so the VM's `run_cleanup` and Pliron's
`lower_cleanup_drops` call it. A storage destroy, a variant store (one
`destructors` entry per alternative, the `__deinit__` the replaced payload
runs under that tag, as upstream's `Variant.__del__` names each under its
tag test), and an owned closure capture (released at the closure's death)
name theirs the same way, from MIR lowering on
(`mir::name_teardown_destructors`, `mir/teardown_calls.rs`, rerun by
`name_destructors` and by a pass that retypes one), respelled by the same
`name_lifecycle_calls`, so the VM's `drop_named` and Pliron's `emit_drop`
call them; a consumed residue's fields and a drop of a pack aggregate's
elements (R1090) still find their destructor from the type
(`mir::destroyed_structs`), as does Pliron's frame-exit sweep of
still-flagged slots (R832); a named drop's fields are its destructor's to
name.

Copies and moves name their constructor the same way, from MIR lowering on:
`mir::name_constructors` (`mir/constructor_calls.rs`) sets each copying or
moving `UseVar`, each `CopyValue`, each owned `MakeClosure` capture, each
`MakeVariant`/`VariantSet`/`VariantReplace` payload store, and each
`MethodCall` or `TryNext` result adapter's `constructor` to the
`__copyinit__` or `__moveinit__` the value's struct records
(`MirDeclarations::struct_member`), as upstream's copy/move insertion leaves
a call naming the constructor at each site. Lowering runs it on every body once the member tables exist (the
`close_register_types` loops of `lower_program` and `lower_package_unit`),
and each later producer of a copy or move reruns it: drop elaboration after
copy elision, `inline-parametric` after a splice substitutes a callee's
types (which names the drops it substitutes the same way, by
`name_destructors`), and the elaborator per instance
(`Specializer::name_lifecycle_calls`). The verifier holds a copy or move to
the same rule as a drop. A borrow, a forward, and a copy or move of a type
whose struct records no such member (a trivial or built-in type, or a
struct whose every field copies or moves without running code) name none
and stay a value copy. A struct declaring none over fields whose copy or
move may run code gets a synthesized `S.__copyinit__(out self, *, copy:
Self)` and `S.__moveinit__(out self, deinit existing: Self)`
(`mir::lower_unit` for `LoweringUnit::{CopyConstructor, MoveConstructor}`,
listed by `CheckedProgram::{synthesizes_copy_constructor,
synthesizes_move_constructor}` where the struct's `IsTriviallyCopyable` or
`IsTriviallyMovable` conformance is possible but not unconditional; the
checker folds a field's conjunct no binder decides, so a generic struct
over `Int` and pointer fields gets none), marked `nodebug` and
`synthesized`, whose body stores each field of the source into `self`'s
through a `value.copy` naming the field's constructor or a `place.move`, as
upstream synthesizes a fieldwise `__copyinit__` and `__moveinit__` for
every struct that declares none; a copy or move of the struct names it and
reaches its fields' constructors through its body. A `ref` field's copy and
move both store the handle the source field holds (`ref.make` of the
source field, `place.store_ref` into `self`'s), since a load of such a
field reads through it; the VM's `MakeRef` of a `ref`-typed place is the
handle that place's storage holds, as the native address of such a place
is the stored pointer's. Captures, variant
payloads, reference-read results, and `T(copy=x)` still find their
constructor from the type (`mir::relocated_structs`, which closes over the
fields only of a struct recording no such member).

A named copy or move is then the call upstream's copy/move insertion
leaves (`mir::lower_copies_and_moves`, `mir/copy_calls.rs`): a `var.use
copy` becomes a `place.load` of the slot and a `Call` of the named
`__copyinit__` passing it as the constructor's borrowed argument, the
shape `S(copy=x)` lowers to; a `var.use move` becomes a forwarding
`var.use` of the slot, which ends it and hands its storage to the callee
as Mojo's owned-argument convention does, and a `Call` of the named
`__moveinit__` passing it as the `deinit` argument; a `value.copy` becomes
the call alone over its register. The constructor's declaration decides
whether the one parameter after `out self` passes positionally or by
keyword (`__init__(out self, *, copy: Self)`); a constructor taking
anything else, or raising, keeps the use. It runs where `lower_whole_drops`
runs: at the end of drop elaboration, after `name_constructors`; in
`inline-parametric` after a splice names the copies it substitutes; and
in the elaborator once `name_lifecycle_calls` has named an instance's
(the template's declaration, found through the instance's key, answers
the parameter), so a site whose type was symbolic in the template is
lowered per instance. Both backends run the call as any constructor call:
the VM constructs through `__moveinit__` as it did through `__init__` and
`__copyinit__` (`lifecycle_constructor`), and Pliron's compiled signature
counts a `deinit` parameter as consuming, so the forwarded temporary
transfers instead of being released after the call. Three uses stay uses:
one naming no constructor, which the backends copy or move by type; one of
a compile-time value parameter's slot, which holds no value and which the
elaborator materializes at each read; and the read of a variadic collector
a call spreads, which the elaborator expands into the pack's element
places (`expand_pack_spreads`, which finds the collector's read in an
earlier block once a pass has split the call's). The parameter-use
analysis reads a call of the constructed type's recorded `__copyinit__`
or `__moveinit__` as it read the copy or move: the constructed value's
arguments at the struct parameters the constructor kept
(`member_instruction`), and `inline-parametric` records in the caller's
removal snapshot each constructor a lowered call names that no spliced
callee listed (`record_lowered_callees`), as the call-graph pass will. A read out of storage and a store copy
nothing in either backend: Pliron's `FnLowering::load_from` takes a byte
snapshot sharing its storage's buffers and `FnLowering::store_to` stores
what the MIR hands it, as the VM does. The instruction
stays a slot use rather than a call, for the drop flags, tombstone, and
`String` bridges the backends attach to it.

Three timing rules complete the last-use model, each pinned against the
current Mojo:

- A call result no expression consumes (an expression statement; the unbound
  result of a non-consuming `__enter__`) is an owned temporary: MIR lowering
  binds it to a hidden `$discard_r` slot that is dead at its definition, so
  its `DropVar` follows the call immediately — before the next statement.
  A non-raising call writes its result in place, as Mojo's `out` slot does:
  the binding right after it is `DefVar { in_place: true }`
  (`produced_in_place`), drop elaboration moves the call's own deaths after
  that binding (`InPlaceDeaths`), and the dead slot drops first. A discarded
  result therefore dies before the call's argument temporaries and the
  owners whose last use is the call; an unused `var x = call(...)`
  declaration is bound the same way. A raising call's result (taken from its
  error-or-value return), a prefix dunder's, an assignment, and `_ = e` are
  separate stores, so the call's temporaries die first.
  A named-struct copy construction (`Name(copy=v)`, `Name[Args](copy=v)`)
  whose whole owned source `v` gets its `DropVar` right after it (or after
  its in-place binding) becomes `v`'s move (`analysis/copy_elision.rs`,
  `elide_dying_copies`, run after `elaborate_drops`), as Mojo's lifetime
  checker turns a copy followed by the source's destruction into a move.
  The checker binds a temporary `copy=` source as a read temporary, so it
  gets a slot and dies there too. A copy construction an owned parameter
  or owned (`var`/`deinit`) receiver takes (`take(M(copy=y))`,
  `M(copy=y).consume()`) is bound in place to a hidden forwarding slot
  (`$fwd_arg_r`, `mir::FORWARDED_ARGUMENT_SLOT`) that the call takes
  through `UseVar Forward` at its setup, as is every other fresh owned
  temporary such a parameter takes; when `y` dies there and nothing from
  `y`'s death to the call can raise or names `y` (`forwarding_use`), the
  load, construction, binding, and `DropVar` go and the call forwards `y`
  itself, so no move initializer runs, as at Mojo, and the slot leaves
  every cleanup list. A raising argument evaluated after the construction
  keeps `y`'s move into the slot, as at the pin. A copy construction a read parameter, read
  receiver, or read operator operand takes (`show(M(copy=a))`,
  `M(copy=a).show()`) is bound to its
  read-temporary slot (`mir::READ_TEMPORARY_SLOT`, below); when `a` dies
  there, nothing from the binding to the call can raise, and only the
  binding and one whole load name the slot (`lending_use`), the load,
  construction, binding, and `a`'s `DropVar` go and the slot's load reads
  `a` itself (`lend`). Every death of the slot — its `DropVar`, a `try`
  body's `cleanup`, an exit's `ReturnWithCleanup`/`EscapeJump` cleanup — is
  then renamed to `a` function-wide, so `a` dies when the call returns, or
  on its raise before the handler, as Mojo binds the borrowed temporary to
  the dead source's storage. A raising argument evaluated after the
  construction keeps the move, as at the pin. A type parameter's
  `T(copy=x)` (still `ConstructTypeParam` at that point), a field source,
  `.copy()`, and a source that dies only on a successor edge stay copies.
  A `_ = e` statement is the same discard: HIR binds its value to a fresh
  per-statement `$discard_s` slot, typed by that value alone and dead at its
  definition, so `_` never names a function-wide local.
- An owning temporary receiver of a method (`B(3).show()`, `print(B(2).get())`)
  gets frame storage in `lower_method_receiver`. One whose declared
  convention is a read carries the checker's `ReadTemporaryArgument` marker
  and is bound like a read argument (below), to a `$tmp_arg_r` slot whose
  load the call takes as its receiver. Any other (a `ref self` receiver, a
  borrowing temporary, a view) is bound to a hidden `$tmp_recv_r` slot whose
  place the call retains, as a named receiver's is. Either way the call is
  the slot's last use, so the temporary's `DropVar` follows the call, before
  the enclosing expression continues. A `var`/`deinit` receiver convention binds no slot;
  the callee owns the temporary. A receiver whose result borrows it takes the
  `$mat_r` path above instead, and lives as long as the borrower.
- An owning temporary at a read parameter (`take(B(2))`) is bound to a
  hidden `$tmp_arg_r` slot (`mir::READ_TEMPORARY_SLOT`) and the call reads that slot, the shape a named
  local takes there (`bind_temporary_argument`); the loaded register retains
  the slot through the call, so the `DropVar` follows it. The checker decides
  which arguments qualify: `read_temporary_arguments` (`checker/places.rs`)
  marks each non-place argument bound to a read convention with
  `ReadTemporaryArgument`, and `record_argument_borrows` marks a method's
  non-place receiver by its declared convention (`read_temporary_receiver`),
  so a `ref self` receiver, which the pin moves into, is not marked; so a `var`/`deinit` slot — which moves the
  temporary into the callee — carries no marker, and MIR never guesses a
  callee's conventions.
- A construction is its `__init__`'s call, so its arguments bind the same
  way: `record_constructor_call` (`checker/declarations.rs`) runs every
  `infer_construction` path through `record_argument_borrows`, so
  `P(S(1), S(2))` destroys each `S` when `__init__` returns and `P(s, s)`
  reads `s` in place. A raising `__init__` records the construction's call
  effect, so it is a raising call: its result is taken after its argument
  temporaries die, and outside `try` or a `raises` function it is
  rejected. Its arguments alias-check as an initializer call too:
  `record_constructor_call` runs the free-call chain
  (`check_free_call_aliasing`) with the constructed type as the callee, so
  `P(s, s)` over `mut a: S, b: S` is rejected, and an argument a `*args`
  collector gathers (`Tuple(Span(xs), Span(xs))`) is judged by its own type
  (`signature_collected_arguments`).
- A struct operator is its dunder's call, so its operands bind the same way
  (`bind_read_temporary`): the checker marks each non-place operand the
  selected dunder reads, the receiver and the argument, a prefix operator's
  operand, a reflected dunder's pair, and a comparison through a bound's
  pair, with `ReadTemporaryArgument`, and `lower_operator_operand` and
  `prefix_op` bind a marked one to a `$tmp_arg_r` slot, so it dies when the
  dunder returns, the argument before the receiver. A chained comparison's
  middles carry no marker: the chain owns them (`chain_middle`). A place
  operand the dunder reads — a named struct, or an element an accessor
  returns by reference (`xs[0]`, `t[0]`) — is marked `BorrowReadArgument`
  instead (`borrow_operator_operand`), and `lower_operator_operand`, which
  `prefix_op` shares, reads it where it lies, as a read call argument is
  read: no copy, so nothing to destroy. An
  operator's result is a fresh temporary as a call's is
  (`Node::is_fresh_temporary`), for this binding and for a field read off it.
- A subscript is its getter's call, so its value category comes from the
  selected getter, not the bracket syntax: over a by-value `__getitem__`
  (`-> T`, a `SelectedCall` whose `reference_result` is `None`) `b[i]` is
  an owned temporary, as upstream. The checker's `Checker::designates_place`
  (`checker/places.rs`) answers place-ness from that fact for every
  consumer that asks whether a value is a place — the read-temporary and
  borrowable-argument filters, the copy demands, the chain middle, the
  `.copy()` receiver, and `^`, which is a no-op on it — and
  `Node::is_fresh_temporary` reads the same contract, so a read argument,
  an operator operand, a method receiver (`lower_method_receiver`), a field
  read, a chain middle (`chain_middle`), and a discarded statement
  (`bind_discarded_result`) bind it as a call result. A `ref` getter, a
  pointer subscript, and a storage read stay places.
- A field read off an owning temporary (`print(B(1).x)`, `var n = B(9).name`,
  `(a + b).v`)
  binds the temporary to a hidden `$tmp_field_r` slot and loads the projected
  place (`load_temporary_field`), as `p.x` does off a named local: the load
  retains the slot through the instruction consuming the field, and a
  consuming context's `CopyPlaceValue` copies an owning field out before the
  slot dies. A reference-typed field keeps the register `GetField`.
- A scalar `LoadPlace` borrows its owner for the one instruction consuming
  the loaded register (the register-loan dataflow's single-hop retention), and
  how long depends on how that instruction receives the register, as the
  callee's declared contract says (`ArgumentPassing`, `register_loans.rs`):
  its declaration's, or for a call through a trait bound in a generic body,
  the requirement the bound trait or a parent declares, found by the call's
  `__trait_dispatch.*` symbol (`MirDeclarations::bound_requirement`), as
  upstream binds such a call to the trait's declared requirement. The
  compiler-known comparison traits, which the bundled library does not yet
  declare, answer from `compiler_trait_declaration`. A
  memory-only parameter takes the argument's address: a generic `T`, a
  variadic collector, or a callee with no declaration (`t.n` fed to `print`).
  Its owner then lives through the call, so the owner's `DropVar` follows the
  call, and owners dying together there drop in descending `VarId`, after
  the call's in-place result binding and its slot, if any. A
  concrete register-passable `read` or `var` parameter, a built-in operator's
  operand, or a slice bound is loaded at call setup instead, in parameter
  order (slice descriptors first), so an owner whose last use it is drops just
  before the consumer, in that load order (`RegisterUses::loaded`).
  `refine_deinit_fields` applies the same split to a `deinit self` receiver's
  fields; a subscript setter's plain indices load like a getter's. An
  aggregate load retains its owner through every consumer as before. A
  moving end of a slot (a `move`/`forward` use, a consume, a whole-root
  move capture) removes that root from every register's provenance: the
  slot's storage backs no register any more, so `two(h^, h.v)` drops no `h`
  after the `forward` has ended it.
- MIR lowering places the load itself at call setup, as Mojo evaluates
  every argument to an lvalue in source order and reads each once the last
  is evaluated. Each call lowerer opens a setup frame (`CallSetup`,
  `open_call_setup`) before its operands and closes it after its last one;
  a by-value operand that is a simple place (`setup_read`: a root or field
  chain with no reference alias, callable, or folded constant) has no lvalue
  work, so its whole read sequence — `UseVar`/`LoadPlace`, a `var` copy, an
  implicit conversion — moves into the frame, and `close_call_setup` emits
  it, the receiver first and then the arguments in the contract's parameter
  order. A by-value operand with a reference result (`setup_reference`: an
  accessor subscript or a `ref`-returning call over a named root) has lvalue
  work: the receiver, index, and accessor or call run where written and
  leave the reference in a hidden `$call_ref_r` slot with setup loans
  (`SetupReference`, `materialize_reference_result_place`), the chain's root
  receiver among them, and only what follows the slot — the `LoadPlace`,
  copy, conversion, and temporary binding — moves into the frame. A static
  call's receiver is a type, never a root, so a static call ends its chain
  as a free call does (`place_chain`). A frame
  belongs only to its own call's immediate operands (`argument_depth`); a
  `mut`/`ref` place, a retained shared read, an accessor chain over a
  temporary, or a read that split the block stays where written. So
  `two(c.t, bump(c))` reads `c.t` after `bump` writes it,
  `two(tup[0], bt(tup))` reads the element `bt` wrote, `two(a.t, take(a^))`
  is a use of the moved `a`, and `two(lst[0], bl(lst))` uses an invalidated
  interior reference. The call consumes its owned operands itself, as
  Mojo's call does: an operand whose lowering ends in a `UseVar Forward` (a
  `^` transfer, a forwarding slot, a conditional's result) has that forward
  moved into the frame's transfers (`CallSetup::forwards`), which
  `close_call_setup` emits after every read, immediately before the call.
  So `two(x^, look(x))` and `owe(k.v, k^)` read the value before it moves,
  and a raise in a later operand finds every earlier owned operand still in
  its slot, where the `try` cleanup destroys it. A display is its constructor's call, so it opens its
  own frame (`display_elements`) over its elements, a dictionary display
  lowering every key and then every value into it, before the collection is
  built: `(u.t, bump(u))` reads `u.t` after `bump` writes it. A list, set,
  dictionary, or array display at a bound position — a call argument, or an
  element of a bound display — is typed by its consumer's parameter, so it
  is built as that argument's conversion: its elements' lvalue work runs
  where written, and its element reads, its construction, and the
  argument's temporary binding move into the enclosing frame like a
  reference result's read (`setup_display`, `display_elements` starting the
  pending `SetupReference`'s read after the elements). So
  `ha([a.t, 1], bump(a))` reads `a.t` after `bump` writes it and
  `ha([lst[0], 1], set0(lst))` uses an invalidated interior reference. A
  tuple display, a comprehension, a display used as a receiver or operator
  operand (`lower_setup_operand`'s `bound` is false), and a display inside a
  nested call keep their natural types and are built where written.
- An element read out of pack storage (an intrinsic `TupleStorage` or
  `VariadicStorage` `Index` with a retained base place) is the same shallow
  read: an aggregate element retains the pack through every consumer of the
  register, a scalar one through the single consuming instruction.
- A consuming parameter (`var`, `deinit`, receiver or not) is a drop root of
  the callee: it is destroyed — for `deinit`, consumed — at its last use in
  the body, and one the body never uses dies at the function's entry, before
  the first statement runs.

- Lowering emits no drops of its own, as upstream's destructor insertion
  owns every destroy. A loop's iterator slot is defined by its `GetIter`
  and dies on every exit edge, `break` included, where it is dropped once.
  A `DropVar` or `ConsumeVar` already in the input is the slot's teardown:
  it is live up to it and dead after it, so the pass splices no second drop
  and running it over elaborated MIR changes nothing.

The VM does not need to discover last uses dynamically. It just executes
`DropVar` where the compiler placed it.

### What Gets Dropped

Drop roots are selected independently of type: every owned local and consuming
parameter receives drop glue. This conservative policy releases heap-backed
runtime storage at its last use even when no user `__deinit__` call is observable,
and it naturally covers destructor-less structs containing aggregate storage.
Ownership is limited to:

- locals
- consuming `var` and `deinit` parameters, receivers included

Borrowed parameters are not dropped by the callee. They are owned by the caller,
as is a `self` outside the leading parameter range of a generated method CFG.

### Drop Order

When several variables die at the same point, they are dropped in reverse
declaration order. Struct destruction runs:

1. the struct's `__deinit__(deinit self)`, if present — its body owns the
   receiver as independently dying direct fields: each field is destroyed
   at its own last use inside the body (an unused one at the destructor's
   entry, before its first statement; a nested projection keeps its whole
   direct field alive; a transferred field is its new owner's), and the
   receiver's `ConsumeVar` at its last use destroys only the survivors
2. otherwise the fields, in declaration order

The compiler-private tuple storage likewise drops elements left-to-right
where it backs a `Tuple`. An owning `var *args` collector — the slot
`MirFunctionDeclaration::owned_pack_slot` names — drops them last to first,
matching current Mojo's pack lifecycle, in the VM and natively. Public
collections, including `Tuple`, are nominal structs and follow the ordinary
declaration order for fields; their library destructors own any
element-specific teardown. Per-field liveness for `deinit` parameters is a
refinement pass after the variable-granular elaboration
(`analysis/field_drops.rs`): for every `deinit` parameter of struct type it
runs a backward liveness over `(parameter, field)` keys — a depth-one field
projection uses that field, a bare receiver touch uses every field, a loaded
register carries its field to every consumer as the register-loan rule does
— with the same fall-off/raise/`finally` seeds and edge splitting as the
variable pass, recursing into `try` regions, and splices `DropPlace` (a
whole-value destruction of one field that tombstones it) after each field's
last use, on block 0 for the unused, and on edges. The receiver's existing
`ConsumeVar` (in-block, cleanup lists, and the native exit backstop) then
skips the tombstoned fields, so raise edges stay sound. Fields that root a
loan, are moved below depth one, or need no destructor work are left to the
`ConsumeVar`. A field read only inside a loop dies after the loop; the pinned
Mojo destroys it at entry (recorded as an `output-diff` row).

A store into a field destroys the value it replaces at the store, as Mojo's
assignment does. A whole-variable reassignment needs nothing extra (the old
value dies at its last use before the redefining `DefVar`), but a field is not
a variable, so a pass ahead of the variable-granular elaboration
(`analysis/store_drops.rs`) splices a `DropPlace` immediately before each
`Store` into a static field or constant-index element of droppable type whose
subtree is initialized there. A whole place written *through a reference* has
no redefining `DefVar` either — its old value is the caller's or another
slot's — so the same pass drops it first: a `WriteRef` through the handle a
`mut` parameter or `mut self` receiver makes of itself, and a `Store` through
a place pointer (`q[] = v`) or a whole-variable `ref` binding. A `WriteRef`
names only its handle register, so the place comes from the `MakeRef` that
defined it. Such a write qualifies only where the whole subtree is intact:
dropping a partially moved value whole would free a hole, and a reference-
rooted place has no leaf flag to guard it natively (reassigning a partially
moved value is a checker error in any case). It replays the move checker's place-tree flow
(`observe_move_states`) from a different entry: parameters initialized, every
other slot uninitialized, and an initializer's `out self` receiver with each
declared field uninitialized, so a constructor's first store into a field
destroys nothing and a second destroys the first. `MarkInitialized`, the image
of upstream's `lit.ownership.mark_initialized`, defines its place in that flow
without writing it: a constructor that then fills the storage through pointers
(`Tuple.__init__`, `Pointer(to=self[i]).unsafe_write({})`) writes through
handles that name no whole variable, so nothing is destroyed, as upstream
assumes the storage uninitialized; the VM and the native backend emit nothing
for it, the receiver's storage already existing. `MarkDestroyed`, the image
of `lit.ownership.mark_destroyed`, is its counterpart: it moves its place in
that flow without reading it, after a consuming member took each element out
through a pointer (`Tuple.consume_elements`), so no destructor runs on the
storage; the VM tombstones the place and the native backend clears its
presence flag. A field wholly moved out is
reinitialized without a drop; a depth-one field moved on only some paths drops
under the VM's tombstone and the native leaf flag. The `DropPlace` touches the
same root at the same position as its `Store`, so liveness, loans, and the
variable pass are unchanged by it. On the VM it follows a reference root (a
`mut` receiver or parameter) into the caller's storage, and equally a handle
the place reaches below its root (the value `p.src[].v = …` replaces lives
wherever `p.src` points); natively a nested or reference-rooted place drops
unguarded at its address.

Types whose `Deinitable` conformance is explicitly unavailable, such
as `Deinitable where False`, are excluded from this automatic path.
A declared conditional `Movable` conformance is likewise effective:
`is_movable` evaluates the struct's own `Movable where ...` predicate, so a
false condition rejects `^` transfers, `var` parameters and receivers, and
move/copy captures at the checker's consuming positions (`check_consuming_as`
distinguishes an ownership `Move` from `Deinit` consumption, so destructors
and named destructors still consume a non-Movable value).
The optional `@explicit_destroy("message")` decorator does not control
linearity; it supplies the required user-facing diagnostic when an obligation
is violated and is inert on an implicitly deletable type. A checked,
stable-binding obligation analysis requires every initialized linear value to
reach exactly one named `deinit self` method on every exit. It rejects
abandonment, overwrite, and inconsistent branch or loop states. A write through
a reference (`ref r = x; r = v`, `same(x) = v`) is an overwrite the checker
rejects at the store itself, since the reference owns no obligation the
analysis could track.
MIR retains the resulting declaration metadata. The checked obligation ensures
that an intact linear value is consumed before it can reach automatic
`DropVar`; the VM therefore does not guess concrete deletability from an open
generic struct name. If any aggregate field has already moved, drop glue skips
the whole-value `__deinit__` for every struct and recursively destroys only its
initialized residual fields.

A named explicit destructor is lowered as a call followed by `ConsumeVar` for a
whole binding or `ConsumePlace` for a projected field. Drop elaboration treats a
pending `ConsumeVar` as the variable's teardown — the variable stays live up to
it and counts as moved there — so no competing ordinary `DropVar` is spliced
between the call and the consumption (which would re-run the whole-value
`__deinit__` the named destructor replaced). The callee owns its `deinit`
receiver from the call on: its own elaborated `ConsumeVar` destroys the
residual fields at the receiver's last use inside the body (moved fields are
tombstones, so nothing re-drops), the VM vacates the caller's receiver place
before the call, and the caller's trailing consumption therefore finds
nothing left to destroy. Consumption is unconditional: a raising named
destructor leaves the source uninitialized on both edges (upstream's rule),
and the checker rejects an `except` arm that re-consumes it.

The analysis follows exceptional exits as upstream's lifetime pass follows
its raise edges. Every potentially raising operation the checker recorded —
a raising call, operator, conversion, or `__iter__`, and a `raise`
statement — is a raise point. An operation consumes the owned operands
evaluated under it (its in-flight linear temporaries and its `^`
transfers) once they all ran, and only then raises, so a raising callee has
consumed its operands while a later operand's raise abandons them. At a
raise point every value still owned must already be consumed: with no
enclosing `try`, all of them; inside one, those declared in the region,
while the rest flow to its target. An `except` arm, or with none the
`finally` body, starts from the join of the states its raise points reach
it with; an arm no raise point reaches is unreachable and unchecked, and a
raise through `finally` continues from its end to the enclosing target.

### Explicit-Destruction Partial Moves

An intact linear binding begins with one whole-value obligation. Moving a field
decomposes that obligation into stable paths for its directly linear child
fields. A moved linear field carries a new obligation at its destination;
ordinary residual fields remain eligible for automatic reverse-order dropping.
The aggregate's whole-value destructor is unavailable while any field is moved,
but a projected linear field can use its own named destructor. Reinitializing all
moved fields reconstructs the intact state and restores the whole destructor.

Obligation paths are part of structured control-flow state. Branches and
loop backedges must agree on both the remaining
obligations and moved fields, preventing a partial state from being hidden by a
join. MIR `ConsumePlace` preserves sibling storage, and the VM drops only
ordinary residual fields of an incomplete linear aggregate.

### Edge Drops

Some values die on control-flow edges rather than immediately after an
instruction. The liveness pass handles these by inserting drops:

- at the end of the predecessor when there is only one successor
- at the start of the successor when there is only one predecessor
- in a fresh split block for critical edges

This keeps ASAP destruction precise across branches, both in a function's
top-level CFG and inside each `try` region's mini-CFG. The block-level
backward liveness, top-level and per region, counts each instruction's
register-loan uses (`RegisterUses`) as the per-instruction replay does, so a
register loaded in one block and consumed in another — a read temporary
loaded before a later conditional argument's branch — keeps its owner live
across the edge until the consuming call.

### Try Region Drops

Region interiors get the same per-instruction and edge drop elaboration as
top-level blocks: each of a `try`'s four regions is a mini-CFG whose liveness
is seeded from the enclosing walk at the instruction, per exit kind — the
normal `TryYield` continuation (`else` entry, then `finally` entry, then the
code after the block), `EscapeJump` edges (the `finally` still runs after
them), the return channel (`RegionSeeds::return_live`: what the `finally`
blocks a return crosses, and those beyond them, still read — the `finally`'s
entry liveness computed against the enclosing return channel by
`region_live_in`, empty at the function's top level), and `EscapeJump`
targets bounded by the enclosing effective live-in. Because a raising operation transfers control to the
raise edge's observer (the handler, or the `finally` and the enclosing
handler when there is none), a **raise seed** — that observer's entry
liveness — is unioned into the live set at every checked raise point
(`scan::raises_to_handler`: `MirInstr::is_raise_point` — a `raise`, a call
whose callee is declared `raises`, or an operator whose dunder raises, which
`UnOp`/`BinOp.raises` records as a call's `raises` — or a nested `try`), as
upstream's CFG has raise edges only at
raising operations and as the move analysis reads them. Backward propagation
therefore places each normal-flow drop after the last raise point preceding
the death: a handler can never observe a vacated slot, and an outer variable
rebound in the body runs the overwritten value's destructor between the
constructing call and the rebind — skipped exactly when that call raises. A
value live into the `try` that no region path can observe (an unconditional
silent rebind precedes every potential raise) dies on the entry edge,
immediately before the `try`. In the enclosing walk a `try` keeps live
whatever its escape targets read (`escape_live_in`): a `break` out of a
handler reaches the loop exit without passing a rebind after the `try`, so
the value that exit reads is live into the `try`, not dead at the loop
body's entry. The loan machinery participates: generation and
register-loan fixpoints run over each region with entry states replayed from
the enclosing walk (regions entered by raise or completion use the union of
every state the preceding region can reach — pure over-retention), so owner
retention and retirement behave identically inside regions.

The enclosing walk places no death after a `try`. Its regions are seeded with
the liveness after it as their fall-off, so each region path that reaches the
fall-off already ends what the `try` uses or holds live for an escape target;
the entry edge, the cleanup lists, and the `EscapeJump` cleanups cover the
rest. A `try` region's entry block has no predecessors (the verifier rejects
a branch to it), so a drop at its head runs once per entry.

Per-instruction drops own all *normal-flow* deaths inside regions, and a
return's too: a value nothing after the return reads dies in-region before
it. A cleanup list stays only where MIR has no block to hold the drop, and
names only what is still held where it runs. A `try`'s list runs on its
body's raise edge alone and keeps the candidates (below) that some raise
point in the body holds — the effective liveness entering each raise point,
and what a nested `try`'s raise carries out past its own list and `finally`
(`TryInterior::raise_held`). A return crossing a `finally` lists what it holds
once those `finally` blocks have run: its live-out less what each crossed
`finally` ends on every path through it (`RegionSeeds::return_ended`); a
return that crosses none lists nothing and is a plain `return`. An escape's
list runs at the `break`/`continue`, before the `finally`, and a handler's
escapes skip the outer values its `try`'s raise edge already ended. One list
over several raise points may still name a slot one of them already ended
(R831); the VM's no-op teardown of a vacated (`None`) slot and the native drop
flag absorb that. The verifier rejects a `drop.var` or `var.consume` of a slot
every path reaching it already ended, and a cleanup list naming one
(`mir::verify`'s slot-end rule), so no pass may emit either.
A `for` loop's yielded slot is in no cleanup list, since the binding always
consumes it.

### Try Cleanup

Try regions need cleanup for the values only the body can still observe. The
drop elaboration pass fills `MirInstr::Try.cleanup` so the VM can destroy them
when a raise leaves the body. Its candidates are the body's *locals* — variables whose every def site
(`DefVar` or `DeclareVar`) in the function lies within the body region (a
reassignment is also a `DefVar`, and a `var x: T` declaration is a
`DeclareVar`, so an outer variable merely rebound or first assigned inside the
body is not a local and survives the exit) — plus the liveness-guarded outer
variables the body rebinds or uses that cannot be observed after the body is
left: dead on the normal continuation, unused by the handler/`else`/`finally`
regions and by an enclosing region's raise and exit observers, and dead at
every escape target (`seed_raise_cleanup`, for a top-level `try` and a nested
one alike); the list keeps those some raise point of the body holds. That
second set covers the raise edge for outer values whose
per-instruction death sits on the normal path: a raise landing before a
rebound value's drop, or before the last use of a value used later in the
body (an owned call operand the call has not yet consumed), would otherwise
leak the value, where Mojo destroys it on the raise edge. A variable a
`UseVar` moves stays listed, since the move empties its slot (the VM's slot
is `Moved`, the native drop flag clear) and the cleanup skips it; only a
variable consumed in place (`consumed_in_place`: a `ConsumeVar`, a
whole-root move capture) is left out. The list is stored in descending `VarId`
order — reverse declaration order, the order of every shared death point and of
`EscapeJump` cleanups — and both backends run it front to back, so a raise
destroys the body's live values newest declaration first, as Mojo does. A slot only
a `try`'s regions define is scoped to the `try`, so the outer drop liveness
does not count a `try` as reading the locals of any of its regions: their
slots are empty whenever the `try` is entered, so a loop's back edge does not
hold them live, and they die at their last use inside their region, the list
the raise edge ending them only when a raise leaves before that. The handler's error binding is one of them: it is a
def site at the handler's entry, where the caught error arrives as the entry
block's argument, and an unread binding dies there.

`EscapeJump` also carries cleanup for cross-region loop escapes. This makes
hidden try-region exits explicit enough for the VM to run destructors before
jumping to the enclosing loop target.

Iterator-driven loops also place an explicit drop at their common exhaustion or
`break` exit. A `return` cannot reach that block, so HIR lists its current
binding and iterator owners on `ReturnWithCleanup`, and MIR keeps each alive
into the return's block (`KeepAlive`) after the return value is evaluated;
drop elaboration then ends them there, or lists them on the return when a
`finally` it crosses reads them (R828 moves their death to the last
`__next__`).

### Pre-Elaboration Pipeline

Upstream runs a pass pipeline on parametric IR after destructor insertion and
before the elaborator. Mojito's is `analysis::PIPELINE`
(`crates/mojito-analysis/src/analysis/pre_elaboration.rs`), an ordered list of
`Pass` rows: a `ProgramPass` rewrites the whole program once and may remove
functions, as upstream's module passes do, a `BodyPass` rewrites each
drop-elaborated parametric body, and a `CallGraphPass` rewrites each body
after its callees, reading their finished bodies through `Callees`, as
MLIR's inliner walks the call graph. `docs/pre-elaboration-optimization-plan.md`
stages the passes that fill it. The list follows upstream's rounds as the
pinned compiler's `--mlir-timing` report shows them, less the passes
Mojito lacks (`SROA`, `CSEPass`). Each of four rounds runs
`RemoveUnusedParams` and `EliminateDeadSymbols`, the second after an
`InlineParametric`; the first then runs `Mem2Reg` and a `Canonicalizer` on
each generator, the second `Mem2Reg`, `SCCP`, and a `Canonicalizer`, the
third an `InlineParametric`, `Mem2Reg`, and a `Canonicalizer`, and the
fourth only `CSEPass`. So `PIPELINE` is `remove-unused-params`,
`eliminate-dead-symbols`, `mem-2-reg`, `canonicalize`; `inline-parametric`,
`remove-unused-params`, `eliminate-dead-symbols`, `mem-2-reg`, `sccp`,
`canonicalize`; `remove-unused-params`, `eliminate-dead-symbols`,
`inline-parametric`, `mem-2-reg`, `canonicalize`; and
`remove-unused-params`, `eliminate-dead-symbols` (R869, R787). `pass_names`
yields a pass that runs more than once once, and all of a pass's runs count
into one `--timings` span.

Before the pipeline, at both seams and whether or not the pipeline is on,
`lower-lit` (`analysis::LOWER_LIT`, `analysis/lower_lit.rs`, R837) erases
the lifetime-only markers, as upstream's `LowerLIT` erases the LIT
ownership ops after `CheckLifetimes`: every `lifetime.keep_alive`
(upstream's `lit.ownership.use`) and every `interiors.invalidate` (state
upstream keeps inside `CheckLifetimes`, with no op) goes, with its marker
register's type and span. Mojito's lifetime check is the ownership
analysis plus drop elaboration, which both read the markers, so they end
right after `elaborate_drops_function`: `CompiledProgram::elaborated_mir`
runs `lower_lit_program` over every body, `__toplevel__` included, and
`finish_package_body` runs `lower_lit` on each package body. Below it
concrete verification and both backends refuse either marker.
`var.declare` stays: it is `lit.var.decl`, which upstream lowers to a
stack allocation rather than erasing, and `mem-2-reg` reads it as the
point where a slot declared in a loop is uninitialized again.

- **`eliminate-dead-symbols` runs in each round** (`analysis::ELIMINATE_DEAD_SYMBOLS`,
  `analysis/dead_symbols.rs`, R778), upstream's `EliminateDeadSymbols`, which
  follows `RemoveUnusedParams` in each round. It
  removes the program's own functions and structs that no root reaches,
  each with its declaration (R834). Upstream's roots are the `export`
  symbols; Mojito's are `mir::entry_roots` (`main`, `__toplevel__`) and the
  functions the compilation exports by name, `Compiler::with_exports`,
  which `pre_elaborate_program` hands each `ProgramPass`: a native caller
  that compiles a root by name from `drop_elaborated_mir` (the scalar
  sweep's `compute`) exports it. `mojito opt` exports nothing. A live
  function makes live every symbol its body or declaration names, its
  overload family, and the functions nested in it; a live struct makes live
  every symbol its declaration names and all its methods, so the
  elaborator's lifecycle, display, hash, and trait-dispatch edges, which
  reach a method from its struct's type, stay live. The uses come from
  `mir::SymbolTable`, MLIR's `SymbolTable::getSymbolUses` over MIR: every
  field that refers to a function or struct (a callee, a selected method or
  operator, a subscript or iterator target, a closure's function, a
  function constant, a compile-time application, a default's evaluator or
  conversion, a struct type or frozen struct value) encodes its spelling as
  a symbol reference (`mojito_types::symbol_ref`, MLIR's `SymbolRefAttr`),
  a newtype every encoding passes through, so package bytes are unchanged.
  The walk visits a value's serialized form and looks up only those
  references (a type is walked once per table); a local, field, keyword,
  literal, or parameter binder that spells a symbol is no use (R835). An
  instruction that calls a bundled body without naming it — `**` on
  integers calls `_pow_int`, a scalar display the digit bodies — uses it
  too, by `mir::implicit_callees`, the rule the elaborator enqueues those
  bodies by. The pass's `--timings` counters are `dead_functions`,
  `dead_instructions`, and `dead_structs`. `emit-mir` and
  `drop_elaborated_mir` therefore omit every dead program function and
  struct.
- **`canonicalize` runs next** (`analysis::CANONICALIZE`,
  `analysis/canonicalize.rs`, R779), upstream's `Canonicalizer` (MLIR's
  `canonicalize`), which upstream runs after `EliminateDeadSymbols`, `SROA`,
  and `Mem2Reg`. It first replaces each instruction
  `MirInstr::folded_constant` folds over operands defined once by a folded
  `const` (`Const::is_folded`) by that `const`, repeating until nothing
  folds (R842), as MLIR's greedy driver applies a `fold` that returns an
  attribute; a result may have several definitions, but every operand has
  one, so each read sees its constant. The replacement is `sccp`'s own
  (`analysis/fold.rs`, as MLIR's `OperationFolder` serves `SCCP` and the
  greedy driver alike), so upstream's first round folds constant operators
  with no `SCCP` in it, and Mojito's first `canonicalize` folds what
  straight-line constants decide. It then rewrites a `branch` whose
  condition has one definition, a `const bool`, to a `jump` to the arm it
  takes, a comparison it just folded included (R781, as
  `hlcf.if`'s `RemoveStaticCondition` keeps the arm a static condition
  takes); the untaken arm keeps its own drops at its entry, so removing it
  ends nothing twice. It then simplifies the control flow (R836), at the
  function level and in each `try` region, until nothing applies: an edge
  of a `jump`, `branch`, or `comptime if` into an empty block that only
  jumps on is threaded to the first block past the chain that is not one
  (MLIR's `SimplifyPassThroughBr`), a `branch` or `comptime if` whose arms
  name one block becomes a `jump` (`SimplifyCondBranchIdenticalSuccessors`,
  as `hlcf.if`'s `HoistYieldResults` and `comptime.if`'s
  `checkTrivialBlocks` erase an `if` whose arms are bare yields), and a
  block a reachable block's `jump` alone reaches is appended to it
  (`SimplifyBrToBlockWithSinglePred`). Upstream needs none of these: its
  generators keep run-time control flow as structured `hlcf` region ops
  until `LowerControlFlow`, after elaboration, so a join or a loop latch is
  no block there; MIR is a CFG before elaboration, and the rewrites remove
  the blocks that structure would not have. The staged move to that structure, which
  replaces these rewrites with `hlcf`'s own canonicalizations, is
  `docs/structured-control-flow-plan.md`. A `comptime for` header's
  targets are never rewritten, since the elaborator finds a loop's body as
  the blocks its `body` target dominates; an escape counts as a
  predecessor and is never rewritten; an edge that would close a cycle of
  empty blocks stays; and a list's entry block is never merged away. Only
  empty blocks are threaded past and only an unconditional jump's target is
  merged, so a drop that drop elaboration put on an edge, and an escape's or a
  return's cleanup, stays on its path. It then erases
  the blocks the entry does not reach, at the
  function level and in each `try` region from its own entry
  (`mir::prune_unreachable_blocks`, MLIR's `eraseUnreachableBlocks`), and
  cuts each `try`'s raise-edge cleanup to the slots a remaining raise path
  still holds (`verify::cut_try_cleanups`, over the slot-ends walk of
  `verify/drops.rs`), as erasing a raise point erases the destructors on
  its edge. It
  then folds each `value.copy` that is the identity to its operand (R780,
  `MirInstr::forwarded_operand`, MLIR's `fold` returning an operand): when
  source and destination are typed alike by a type
  `MirDeclarations::copy_is_identity` accepts, the copy is removed and every
  reader names the source, chains resolved to their root. That is a value
  held whole in one register on both backends (`types::copy_is_identity`: a
  scalar, pointer, `None`, or one SIMD lane), or a register-passable trivial
  one, which upstream's parser loads into an SSA value its copies are: a
  compiler-known type upstream declares `TrivialRegisterPassable`
  (`types::trivially_register_passable_leaf`: vector SIMD, `StringLiteral`),
  or a struct declaring it unconditionally with no `__copyinit__` or
  `__del__` and only such fields; a tuple is not one, as upstream's is not.
  Only a result with one definition whose source has at most one is
  forwarded, so the source's definition dominates every reader, and a copy
  of a value held in storage only when it is its source's sole reader, since
  a reader may consume it. A `place.load` result is the place's value at the
  read: a backend may alias it to the place (Pliron's
  `collect_aliased_load_regs`) only for a type whose copy is not the
  identity, while nothing writes the place before the register's last
  reader. Then, until nothing
  changes, it erases each instruction whose results nothing reads and
  that `MirInstr::would_be_trivially_dead` declares free of effects but
  reading (MLIR's `wouldOpBeTriviallyDead`): constants, literal
  materialization, layout and name queries, borrows, field and reference
  reads, plain place reads, variant tests, and SIMD value operations. As in
  MLIR, anything not declared free of effects stays: a copy that is not the
  identity (it may run `__copyinit__`, which only an instance decides), a
  move, an operator (a
  dunder call per instance), a call, every drop, consume, and ownership
  mark (none of which defines a result), and every loan or interior marker.
  A register's read count is every register its instruction names but its
  own results, so an operand no enumeration classifies keeps its definition.
  An erased result loses its type and span; registers are not renumbered.
  Erasure can empty a block and a folded branch leaves its condition
  unread, so the control-flow simplification and erasure repeat until
  neither changes anything.
  Its `--timings` counters are `folded_constants`, `folded_branches`, `threaded_branches`,
  `folded_successors`, `merged_blocks`, `erased_blocks`,
  `forwarded_copies`, and `erased_instructions`.
- **`inline-parametric` opens the second round and runs again in the third**
  (`analysis::INLINE_PARAMETRIC`, `analysis/inline_parametric.rs`, R784),
  upstream's `InlineParametric`. It follows the callees whose declaration's
  `InlineLevel` is `NoDebug`, `Builtin`, or `Always`
  (`is_inlined_before_elaboration`), and replaces a call to one with the
  callee's finished body, drops already inserted: a `nodebug` or `builtin`
  callee at any size, as the pin inlines it, and a plain `@always_inline`
  callee only while its body's `inline_cost` is below `INLINE_SIZE_BOUND`
  (32), the pin's count of the operations its first round leaves,
  calibrated on probe bodies: a slot's reads, stores, and declarations, a
  copy or drop running no constructor or destructor, and a repeated
  constant are free, a `branch` costs one as the `hlcf.if` it came from, a
  loop's back edge four beyond its test and body, `iter.init` the whole
  bound as the pin's `range` protocol does, and `print` the 28 its call
  site costs there; each other instruction costs one. The callee's blocks are
  appended to the block list holding the call, the function's or a `try`
  part's, with its registers and slots moved past the caller's
  (`mir::renumber_block_slots`); its slot names gain `$inline{n}`, so no
  binder name or `self` matches them. The call's block binds each owned
  parameter, each `read` parameter of a scalar type
  (`mir::has_no_destructor`), and each `read` parameter passed a trivial
  copy of a closed-type slot (a `var.use copy` naming no constructor), with
  a `var.store` of the argument into the parameter's fresh slot, and every
  other `read` parameter is the caller's place the call retained for it or
  the argument was `place.load`ed from, directly or through a `ref.read`
  of a `ref.make` the same block made (the private `loaded_place`),
  composed into each of the callee's places rooted at it, as upstream
  substitutes the borrowed address. A whole-slot copy of a place-bound
  parameter becomes a `place.load` of the caller's place and a
  `value.copy` of the loaded register (the private
  `copy_bound_parameters`, taking a fresh callee register), which
  `name_constructors` then names the instantiated type's `__copyinit__`
  as it names any copy; an `iter.init` over one iterates the caller's
  slot, and a loan's interior of one is an interior of that slot; a
  parameter bound so that the callee names it otherwise as a slot keeps
  the call (R1099). The callee's one returned register becomes
  the call's; several returns store to a `$inline{n}$result` slot the
  call's register moves from, and a `None` result is a constant. What
  followed the call moves to a new block every return jumps to, and an
  `in_place` binding of the call's result drops the flag. Every inlined
  register takes the call's location with no origin slot, which is what
  `nodebug` asks for and keeps the native debug attach's per-function call
  correlation in the caller's file. A callee with its own compile-time
  parameters is bound to the call's `instantiated_args`, one per binder in
  declaration order, before its body is spliced: the call's arguments
  replace its binders through every site `mir::visit` reaches
  (`types::replace_parameters` and its siblings, with
  `types::replace_constraint_parameters` for a `comptime if` condition),
  as upstream's `InlineParametric` substitutes through MLIR's
  `AttrTypeReplacer`. An argument over the caller's own binders stays
  symbolic, for the elaborator to fold in each instance of the caller. A
  value binder's slot reads become its argument as a constant
  (`Const::of_parameter_value`, or `Const::Param` while symbolic), and the
  slot's drops are deleted. A callable value binder
  (`f: def(Int) thin -> Int`, a `value_param` of function type bound to a
  deferred value) is a runtime slot of the body the VM reifies from the
  call's `param_arg_regs` entry, aligned with `instantiated_args`; the
  pass resolves that register through `mir::callable_targets` over the
  caller, or the `const function` the block defines, and binds the slot to
  the function as a constant (the private `SlotValue::Function`), deleting
  its drops and clearing a `call.indirect`'s `callee_place` rooted at it.
  A register naming a closure of reference captures the caller makes binds
  the slot to that closure (`CallableArgument::Closure`): the inlined entry
  stores it there at the closure's type, so the body's call through the
  slot inlines as a call through a caller's closure, and the call's
  `capture_accesses` are that closure's (`accessed_through`); one naming a
  closure with a copied or moved capture, or nothing, keeps the call. A
  body declared in a `comptime for` inlines where the binders of its
  regions are in scope: the caller's own, or a `comptime for` enclosing the
  call (`Inliner::unscoped_region_binders`, `LoopBodies`), as upstream
  inlines a generator in the loop it unrolls; the elaborator then
  instantiates a body nested in it from the caller's unrolled copy
  (`Specializer::region_binders_up_to`). A callable-bound type binder binds as any type. An origin
  argument binds no binder (the private `without_origins`): the MIR
  declaration's binders are its type and value parameters, and the body's
  origin parameters stay symbolic in the caller as in an instance. A
  binder's constraints, the callee's `where` clauses, and its
  `availability` bind nothing: the checker judged them at the call, as the
  pin rejects a call it cannot prove with "lacking evidence to prove
  correctness", so the inlined body carries none. A `comptime for`
  terminator passes `is_closed`, and its `ComptimeSequence` closes over the
  bindings (`ComptimeSequence::try_map_exprs`), its loop binder staying the
  body's own for the elaborator to unroll in the caller. The binding keeps
  the call for a value only the elaborator folds (a deferred or marker
  value), a type still
  naming a bound binder after substitution (`types::names_binder`, which
  counts a pack query), a `T()` or `T(copy=x)` at a concrete `T` (R1097),
  a conformance test of a concrete binding (R1096), or a value binder's
  slot used other than as a read. A compile-time application over a bound
  type pack (`Tuple.reverse`'s `comptime for` bound, a thunk applied to
  the owner's `Ts`) is applied to the bound element list, which
  `Specializer::demand_application` binds to the thunk's variadic binder
  as the pack's runtime type (R1079), and a non-variadic type binder's
  `DeclRef` in such an application binds to its argument as a type shape.
  A method of a parametric struct binds
  the struct's binders to its receiver type's arguments, and a call naming
  a struct whose one `__init__` is followed binds them to the constructed
  type's, past the binders the initializer restates; the member's bare
  spelling of its owner becomes that type, and a read of an owner value
  binder through `self` (`Self.k`, a member load) becomes its argument as
  a constant. A struct keyed on one pack applied to no element (`Tuple()`
  outside `Tuple`'s own members, where the bare spelling denotes `Self`)
  binds the pack to the empty list (R1093). A type whose arguments do not
  align with the binders keeps the call. The
  initializer's `out self` is a fresh slot that an
  `ownership.mark_initialized` opens and the call's register moves from
  after the body. Callee registers are renumbered before a place-bound
  parameter is composed, so the caller's index registers in that place
  stay the caller's. A `call.indirect` through a closure the caller makes
  (the private `Closures` table: a `closure.make` register, or a slot
  stored once from one) calls the nested `def` the closure lifts. Its
  leading parameters, the captures, bind to the places the closure
  captures by reference, as a `mut` parameter binds the caller's place,
  and a read of an enclosing declaration's value binder, which the lifted
  body spells as a slot of its own, becomes a `Const::Param` reference to
  that binder, which the caller, a body in its scope, binds. The
  call-graph walk follows a `closure.make` as an edge, so the nested body
  is finished first and a nested `def` that makes its own closure lies on
  a cycle. The callee must have no region binders, a struct receiver if
  it has one, be called with no keywords collected afresh into
  `**kwargs` (the dictionary the call builds, R1070) and, unless through
  a closure, no capture accesses, capture
  nothing by copy or move, return a value rather than a reference, and
  hold plain control flow with no `try`, region op, or closure; any other
  call stays a call. The call's positional and keyword arguments bind to the
  callee's regular parameters and collectors through
  `mojito_ast::call::match_call_slots`, the matcher the VM's `bind_args`
  applies, the bindings laid out by `FunctionDeclaration::slot_parameter`
  as the VM lays out the frame; a call it does not bind stays a call. A
  positional collector (`*xs: Int`, `*args: *Ts`) is a fresh slot of the
  collector's instantiated type that the call's block initializes from a
  `tuple.make` of the overflow registers (`Binding::Collector`), typed
  `$variadic[T]` for a homogeneous collector and the elements' tuple for a
  heterogeneous one bound to a list (`types::replace_parameters` turns
  `VariadicPack(*Ts)` into `Tuple(elements)` as `expand_packs` does in an
  instance), which the VM runs as the `Value::Tuple` its binding would
  build and Pliron lowers to the pack descriptor (`lower_make_tuple`); a
  `read` collector's slot is never dropped, its elements being the
  caller's, and an owned one's drop is the body's own, so Pliron's
  frame-exit backstop skips pack-storage slots (R1102). A spread
  (`show(*args)`, `take(*args^)`) binds the collector as the one argument
  `mojito_ast::call::bind_spread` checks it is: the caller's place for a
  `read` collector, the moved register for an owned one; a spread the
  inlined body itself keeps, now typed the bound elements' tuple, is left
  for `expand_pack_spreads` (`verify_pack_spread` admits it, R1103). A
  forwarded `**kwargs^` (the `**` keyword, `FORWARDED_KWARGS_NAME`) binds
  the keyword collector's slot whole as an owned argument, the matcher
  never seeing it, as the pin binds a forward to another `**kwargs`
  alone; keywords collected afresh keep the call (R1070). Each omitted argument
  binds a fresh slot that the call's block initializes from its default
  (the declaration's `CheckedConst`), materialized as upstream's parser
  materializes it in the caller: a literal as a `const`, crossing the
  literal boundary with a `literal.materialize` at a scalar parameter, or
  a converting constructor's `call` (naming the struct for its one
  `__init__`) run on such a literal. The slot is dropped after the body
  when the parameter only borrows a type with a destructor. A default
  computed by a lowered default function (`CheckedConst::Evaluate`), or a
  literal the parameter's type does not hold, keeps the call. A raising
  callee's `raise`s and raising calls stay raise
  points of the inlined body. Inlining into a `try`'s body adds to that
  `try`'s raise-edge cleanup each inlined slot with a destructor that some
  inlined raise point may hold (a forward walk from the bound parameters'
  slots, a slot held from an instruction naming it until a drop, consume,
  or whole-slot move ends it, as `verify/drops.rs` counts ends), so the
  callee's locals end on the raise edge as its own frame ends them at the
  pin; the handler, `else`, and `finally` raise past it to the next
  enclosing `try`. An initializer's `out self` is never listed, being
  partly initialized at a raise. Outside any `try` the slots leave the
  function held, as every raise leaving a function does (R20). A
  destructor's call at a whole slot's drop (the `MethodCall`
  `mir::lower_whole_drops` left, or the one the dropped default's slot
  gets) is an ordinary call of the pass: its `deinit self` binds the
  slot the receiver register forwards (`forwarded_slot`, `Binding::Slot`,
  the slot renumbered to the caller's, the forwarding `var.use` removed
  by the splice), so the body's `consume.var` ends it as the forward did,
  and the struct's binders bind to the slot type's arguments as a
  method's bind to its receiver's. A `deinit` parameter passed a
  whole-slot `place.load` (an explicit destructor's receiver,
  `t^.reverse()`) binds that slot too, the splice removing the caller's
  `consume.var` of its residue after the call, which the body's own
  consume now is, and one passed any other register (a copied receiver's
  `__moveinit__` result, `pair.reverse()`) is a fresh slot the register
  initializes for the body to consume (R1079). A `drop.var` the lowering
  kept, whose slot some path leaves partial or empty, is no call and is
  never expanded. A callee on a cycle of followed calls is never inlined: at
  the program seam the cycles are the strongly connected components of the
  program's followed calls, and at the package seam `PackageBodies::on_cycle`
  decides one from lowered bodies, memoized by row, so a finishing hook
  never re-enters the body it finishes.
- **`mem-2-reg` runs before each round's `canonicalize`**
  (`analysis::MEM_2_REG`, `analysis/mem2reg.rs`, R787), upstream's
  `Mem2Reg`, which promotes an allocation whose every user is a load, a
  store, or a lifetime marker. A slot is promoted when its type is
  trivially register-passable at a concrete size
  (`MirDeclarations::copy_is_identity`, never symbolic: upstream promotes
  only allocations of concrete size), it is no parameter, and every
  mention of it lies in one block list (the function's own or one `try`
  part) and is a `var.store` of a register defined once, earlier in its
  block, and read by the store alone; a `var.use` (copy, move, or forward)
  or a whole-root `place.load` at the slot's type; or a `var.declare`,
  `drop.var`, or `var.consume`, all of which do nothing to such a type
  once drops are elaborated.
  A destructor or copy constructor takes the slot's address, so its type is
  never one, as upstream's never is; a borrow, capture, loan, projection,
  iterator, or cleanup list keeps the slot. A slot every store of which is
  a `closure.make` of reference captures is promoted at any type: that
  closure is a struct of pointers, trivially register-passable as
  upstream's is, and a `call.indirect` through the slot keeps it;
  `canonicalize` then forwards a `value.copy` of such a closure and erases
  the unread `closure.make` (`MirInstr::would_be_trivially_dead`). A forward dataflow over the
  list's blocks (a `try`'s escapes taking the state at the `try`) finds
  each read's reaching store; when every read sees exactly one, that store
  lies on every path to the read and its register's definition before it,
  so the read becomes a `value.copy` of the stored register, the other
  mentions go, and the slot leaves the function (`mir::retire_slots`). The
  `canonicalize` after it forwards the copies. A slot whose reads merge two
  stores keeps its slot (R927): upstream carries such an allocation through
  `hlcf` region arguments and results, and MIR is a CFG at this seam, whose
  blocks take none. The elaborator copies a `comptime for` body once per
  iteration with every register renumbered (`mono/unroll.rs`
  `renumber_block`), so a read and its store must lie on the same side of
  every loop body's boundary (`mir::comptime_for_body`); the guard goes when
  the elaborator clones regions (R880). Its `--timings` counters are
  `promoted_slots` and `erased_slot_instructions`.
- **`sccp` runs between the two `canonicalize` runs** (`analysis::SCCP`,
  `analysis/sccp.rs`, R781), upstream's `SCCP` (MLIR's sparse conditional
  constant propagation). Each register takes a lattice value, unknown, one
  constant, or overdefined, over the blocks the entry makes executable: a
  `branch` makes only its taken arm executable while its condition is one
  constant, a `comptime if` or `comptime for` both of its targets, and a
  `try` its regions, whose escapes make their function-level targets
  executable. A scalar `const` (`Int`, `Float64`, `Bool`, `IntLiteral`,
  `FloatLiteral`, or a `UInt` or SIMD `Const::Value`, `Const::is_folded`)
  is its constant; a parameter read
  (`Const::Param`) and an aggregate `Const::Value` are overdefined, the
  first the elaborator's to fold per instance. An
  instruction is the constant `MirInstr::folded_constant` (MLIR's `fold`
  returning an attribute) gives over its operands' constants, unknown while
  an operand is, and overdefined otherwise. The fold covers built-in
  operators with no checker-selected dunder and no `raises`, `not`, `-`,
  and `~`, a literal materialized to `Int`, `UInt`, `Float64`, or a
  one-lane SIMD type, a one-element `simd.make` of a literal at any known
  width, and above one lane a `simd.make` with one element (upstream's
  splat `SIMD.__init__`) or one element per lane (upstream's
  `SIMD.__init__(*elems: Scalar[dtype])`), each element a literal, a
  one-lane constant of its dtype, or a `Bool` into a `bool` vector, a built-in scalar conversion
  (`MirInstr::scalar_conversion`: a bare `call` of `Int`, `UInt`,
  `Float64`, or `Bool` with one argument that names no declared function,
  as the VM and Pliron resolve it, Mojito's counterpart of the
  `pop.cast` or compare upstream's inlined `SIMD.__init__` and
  `Bool.__init__` leave), and an identity
  copy, over values whose constant kind is the one their register's type
  names. A conversion folds by `CtValue::folded_conversion` over
  `ct::convert_lane`, the rule the VM's `runtime::builtin_convert` and the
  comptime evaluator call too, and folds nothing for a float converted to
  `Int` or `UInt` that is out of range or not finite, which the pin's
  interpreter refuses and only run time saturates. A materialization or a one-lane `simd.make` folds to the value
  `CtValue::materialize_as` gives, and a wider `simd.make` to the lanes
  `ct::simd_construction_lanes` gives, each element's lane by
  `CtLane::from_value`, the construction rule the comptime evaluator and
  the checker's constraint evaluator share; an element Mojito narrows but
  the pin rejects (an `Int`, divergence R186) folds nothing. The literal lane rule
  (`CtLane::of_int_literal`, `CtLane::of_float_literal`) is the VM's own,
  which `runtime::materialize_literal` and `runtime::simd_from_values` call;
  a `UInt` or SIMD result is a `Const::Value`, upstream's
  `kgen.param.constant` over a typed attribute. The operator fold is
  `param_expr::fold`, the one compile-time folder. Over SIMD lanes it
  computes by the VM's own lane rule (`ct::{int_lane_binop,
  float_lane_binop, lane_compare, bool_lane_binop}`, which `simd_binop`
  calls), as upstream's `pop` ops fold their `#pop.simd` attributes: a
  literal or a one-lane operand splats, an integer lane's zero divisor
  gives a zero lane, a narrow float lane rounds once, and a comparison
  folds to a `bool` mask. It folds nothing where that folder and the VM
  could disagree (a zero scalar divisor, a scalar shift of 64 or more, a
  lane shift outside the lane width, an overflowing `**`, a NaN or
  infinite scalar comparison, `Float64` `//`, `%`, and `**`, float-lane
  `//`, `%`, and `**`, `Bool` operators but `==` and `!=`).
  The rewrite replaces each instruction that folds over the final lattice
  by a `const` of the same register, which keeps its type and span, through
  the replacement `canonicalize` shares (`analysis/fold.rs`); the first
  `canonicalize` has already folded what straight-line constants decide,
  so `sccp` adds the folds that need its executable-block reasoning or a
  lattice meet. The pass never rewrites a terminator, which the
  `canonicalize` after it does.
  Its `--timings` counter is `folded_constants`.
- **`remove-unused-params` runs in each round** (`analysis::REMOVE_UNUSED_PARAMS`,
  `analysis/unused_params.rs`, R782), upstream's `RemoveUnusedParams`. It
  records on each body, as `MirFunction::removed_params`, the function's own
  compile-time parameters (`ParamId::owner` is its lowered name) that
  nothing in its body or signature reads, `mir::unread_parameters`'s
  answer. A read is a binder identity anywhere in the serialized blocks,
  slot and register types, signature, runtime defaults, `where` clauses,
  and the other parameters' types and constraints, found by a serde walk
  as `SymbolTable` finds symbols (MLIR's walk of every attribute and type);
  a struct type, though, reads only the arguments its lowered layout reads
  (R977), as upstream's walk runs after `LowerLIT` made each struct the
  structural type of its fields (`mir::StructReads`,
  `mir::struct_layout_reads`): a field type reads what it spells, a struct
  field only what that struct's layout reads, and a pointer field nothing
  of its pointee, so `fb[n](b: Box[n])` over `Box[n]{v: Int}` removes `n`
  while `Arr[n]{data: SIMD[int64, n]}` and `Holder[T]{x: T}` keep theirs.
  A struct type is read whole where upstream's IR keeps it nominal: a
  declared callee's compile-time arguments and receiver, a constant, a
  compile-time condition or sequence, a call of an undeclared callee that
  may run a member of a struct it names (a constructor, R1020, or the
  builtin `len`, R988), and the parts of a dropped, copied,
  or moved value's type its lifecycle member reads
  (`StructReads::member_reads`). Upstream's destructor and copy/move
  insertion leaves a call of that member, which passes the struct's kept
  parameters as values, so a struct declaring the member reads the
  arguments at the parameters it kept, whole, when its removals are at
  hand (`StructReads::with_members` over the callees' removals, R999) and
  every argument otherwise, and one that does not reads what its fields'
  members read, as upstream synthesizes it. Each declared member a drop,
  copy, or move runs (`mir::lifecycle_callees`) is a call-graph edge, so
  it is finished before the body running it, and joins the
  `callee_removed_params` snapshot, empty removals included
  (`StructReads::consulted`). A call of a struct's member (a resolved
  method call, a subscript call, a resolved operator, a prefix `-`/`~`,
  which the check resolves to its dunder and MIR lowers to that method
  call, and an iteration step) passes the member's parameters as values on
  the pin, so it reads the receiver's arguments at the struct parameters
  the member kept, whole, and its own recorded parameter arguments, while
  its operands and result read their layout (R998); this needs the
  member's removals at hand (a unit body off a cycle, followed callee
  first), a receiver spelling every parameter of the member's struct, and
  the member's kept own parameters recorded on the call, and falls back to
  reading every type the instruction names. The callee joins the caller's
  `callee_removed_params` snapshot, empty removals included, which the
  verifier reads the call through (`mir::UnreadParameters::member_callees`).
  A value argument, a dependent type, and a
  projection read their binders whole as well; a
  value parameter's slot (matched by name, as the elaborator fills it)
  addressed by anything but a drop or declaration of a scalar or of a
  callable parameter, whose value is a compile-time one; a name of
  a body lifted from the function (`$comptime$f$…`, `$default$f$…`), which
  reads the binders it declares that its own run kept (R982); and a name
  of a body nested in it (`f$…`), which reads the parameters that body's
  own run kept (R908). A nested body is
  a generator over its enclosing scope, as upstream lifts a closure to
  one: its candidates are its enclosing declarations' parameters and the
  binders of the `comptime for` regions each is declared in
  (`region_binders`, R974), which it records as `removed_params` when it
  never reads them, and the elaborator keys its instances on the rest
  (`Specializer::nested_bindings`, which the unroller's region copies
  name their instances and generic generators by too). An application of
  a lifted body (a `comptime for` bound's `$comptime$f$k`, applied to
  every binder in scope) passes no read at a binder that body removed, as
  upstream rewrote the application without it, and `verify/scope.rs`
  keeps a removed binder in scope there, by the body's
  `callee_removed_params` snapshot. A
  method is a generator over its struct's parameters as well, as
  upstream's method generator lists them before its own
  (`mir::method_owner_params`, R909): they join its candidates. The
  struct spelled with no arguments (a receiver, `out self`, or `Self`
  result) reads them as the struct spelled at its own parameters does
  (R978): where a struct type reads its layout, the ones its layout reads,
  as upstream walks `self`'s lowered structural type, and where it reads
  nominally, all of them; the lifecycle member a copy, move, or drop of
  `Self` runs reads what it reads of `S[own parameters]`
  (`Walker::member_reads`), and `Self.n`, lowered as the projection
  `self.n` of the receiver's reified parameter, reads `n`
  (`receiver_parameter_reads`). So `get(self)`, `take(var self)`, and
  `bump(mut self)` over `Box[n]{v: Int}` remove `n`. A method that removed
  one is keyed and named off the struct template with its kept struct
  arguments leading its own (`S.st$mono`, `Specializer::enqueue`), so it
  names no owner instance, and the bare spelling in its body stands for
  the struct with the removed arguments erased
  (`Specializer::erase_removed_owner`, `Box$mono$V$REMOVED$ARG`), which
  every receiver's instance agrees with. A destructor, copy, or move
  constructor is such a generator too: each concrete struct declaration's
  member table (`StructDeclaration::members`) names the instance its drops,
  copies, and moves run, which the elaborator records as it demands it
  (`Specializer::record_struct_member`), so `DBox[1]` and `DBox[2]` share
  one `DBox.__deinit__$mono`, as upstream's struct nests its member
  generators. The table names each member a backend reaches from a
  value's type rather than from the instance a call names, too
  (`mir::DISPATCHED_MEMBERS`: display, a writer's `write_string`, hashing,
  equality, `__int__`, a keyword collector's `__setitem__`, `__call__`),
  keyed by the template's method spelling, overload qualifier included,
  and the backends select among a member's entries with
  `symbol::MemberCandidates`, the overload rules of
  `resolve_callable_symbol`, so `WBox[1]` and `WBox[2]` share one
  `WBox.write_to$mono`. It also names every other instance of a member the
  struct's type reaches, as upstream's conformance witness tables do: a
  special method or a requirement of a trait the struct declares
  (`mir::MirDeclarations::reached_from_type`, `Specializer::table_key`,
  R1006). Those stay live through the calls that name them. A VM dispatch with
  no selected instance of the receiver's own struct (an operator, a
  protocol method, a trait requirement, a type-reached dunder) runs the
  instance the table records under the selected method spelling, else
  the one callable resolution picks among the member's entries, and
  never composes `{struct}.{method}` (`Prog::runtime_method_name`,
  `VmBackend::call_resolved_dunder`, R1027). A construction names the constructor the check selected from
  lowering on, as upstream's call names the selected `__init__` generator
  (`overload_support::constructor_symbol`, R1020): `Box[n](5)` calls
  `Box.__init__` with the struct's arguments as that constructor's, since
  its declaration restates the struct's parameters, so a constructor that
  removed `n` leaves a construction that passes none; an `@implicit`
  conversion passes them the same way (`ConversionInstantiatedArguments`),
  and `T(copy=x)` over an explicit copy constructor calls
  `T.__copyinit__` with the spelled `T[n]` as its receiver, as a static
  call carries it. The elaborator respells each to the instance it
  reaches, and also every call it makes itself that names a struct,
  `make[String]()`'s `T()` included, so no backend resolves an overload by
  arity for it; only a fieldwise struct with no `__init__` and a
  synthesized `T(copy=x)` keep the struct's name. A constructor that removed one of its
  struct's parameters is owner-less like any other method, and the VM builds the call's destination
  type through it (`Prog::lifecycle_constructor`, `CallTypes::constructed`),
  so `Box[1](1)` and `Box[2](2)` share one `Box.__init__$mono`. The two
  constructors a backend runs with no call naming them, a keyword
  collector's empty one and a slice bound's `Optional` ones, are member
  table entries (`mir::CONSTRUCTED_MEMBERS`) their demand sites record. A static call's spelled receiver `S[n]` is a
  forward of `n` to the struct parameter the callee removed; in a body
  that removed `n` it is the erased `S[_REMOVED_ARG]`, which binds nothing
  and names the erased instance (`Specializer::bind_static_receiver`). A body lifted from a compile-time
  expression (`$comptime$f$…`, `$default$f$…`) is a generator over every
  binder it declares, whoever owns it (R911): its candidates are its
  declaration's `param_decls`, and the elaborator keys its instances, and
  the evaluations cached by instance name, on the binders it kept
  (`Specializer::{demand_application, default_function_bindings}`), still
  binding a removed one, as a call's instance does. The pass follows
  nested and lifted bodies, a closure or function value and a lifted body
  a body or its signature names (`mir::named_lifted_bodies`) count as
  call-graph edges, so either runs before the function naming it, and its
  removals join that function's `callee_removed_params`, a lifted body's
  even when empty; one whose run is not at hand reads every parameter. A
  runtime default is no read, since every call omitting it evaluates it at
  the call (`mir::fill_omitted_arguments`), as upstream's callee names no
  default (R1005). A nested function's candidates are its own parameters
  beside its enclosing declarations' (R973), and the binders of the
  `comptime for` regions it and each of them is declared in (R974).
  `__toplevel__` has no candidates. A callable value parameter is one
  like any other (R910): the elaborator binds no runtime closure or
  environment for one the callee removed
  (`Specializer::bind_capturing_callables`), and a call site with no
  compile-time value for it binds nothing.
  Each run records the unread set afresh, and a parameter only grows
  removable as the cleanup passes erase reads, so the last round's run,
  after every cleanup pass, decides it. The elaborator leaves a removed parameter's
  argument out of the instance key wherever it walks the template's own
  `param_decls` (`Specializer::own_arguments`), so instances that differ
  only in it are one, and names an instance whose template removed any
  parameter by `instance_symbol` even when no argument is left (`f$mono`).
  Every caller still binds every parameter. Upstream instead rewrites the
  generator's signature and its callers; Mojito keeps both, and an
  instance clears the field. The verifier takes a removed binder out of the
  body's scope but inside a struct argument, which an instance erases
  (R996), and reports a removed parameter the body reads.
  Before the parameters, the pass removes the runtime arguments the body
  never uses (R997), upstream's `identifyUnusedArguments`: it records as
  `MirFunction::removed_args` each parameter slot that no instruction,
  handler binding, or cleanup list names but to drop a value whose drop
  runs no code (`mir::used_slots`, `MirDeclarations::drop_runs_no_code`,
  upstream's `IsTriviallyDeinitable`), and reads the body and declaration
  with them gone (`mir::without_removed_arguments`,
  `mir::retire_parameters`, `mir::unread_parameters_of`), so a parameter
  only an unused argument's type named is unread too: `fa[n](a: Arr[n])`
  and `fav[n](var a: Arr[n])` remove `a` and then `n`. Only a free
  function, a nested one, and a method (R1011, R1032) that is neither a
  `mir::table_member` nor a protocol member a backend runs from a value's
  type with every argument (`__iter__`, `__next__`, and the `__len__`,
  `__abs__`, `__bool__`, and other dunders the builtins dispatch), none
  declared in a compile-time region nor tying an origin or a returned
  reference to a parameter, has candidates (`mir::unused_arguments`), and
  never a `deinit` or a callable parameter, nor a subscript accessor's
  receiver, which every subscript passes (R1109). A slot maps to its
  parameter by the body-slot layout
  (`FunctionDeclaration::slot_parameter`): a removed method slot past the
  receiver is the declaration's position one lower, and the resolved
  `MethodCall` arm drops its argument as the `Call` arms do. An operator
  on a struct operand is a `MethodCall` of its dunder by then, so it drops
  the operand too (R1033), and a subscript names its accessor's reduced
  generator through `rewrite_subscript_call` (`ReceiverCall::reduced`):
  its contract loses the arguments the removed slots bound, a
  multi-subscript sheds the operands they named, and an `index.get` or a
  slice whose one operand goes becomes a multi-subscript with none, which
  both backends run as the same call over the contract. `inline-parametric`
  reads an inlined callee's reduced generator through
  `with_removed_argument_forwards_erased`, as `Specializer::materialize_body`
  reads an instance: the callee's own forwards to its callees' removed
  arguments are erased before its arguments go, and the operands stay for
  the elaborator to drop at each call. A borrowed collector is a candidate too (R1014): the reduced
  declaration collects nothing, its call drops every overflow operand of
  that kind, and `infer_call` still binds the parameters the elements
  spell but keys no arity, while an owned `var *args` or `**kwargs` is
  used by its drop, which runs code. A nested body's captures are its
  leading arguments, candidates like any other (R1014). A call of a
  nested body by name is a direct call of it (R1037), which names the
  reduced generator and drops the removed operands as any direct call
  does; only a body a closure or function value names (a function value
  passed on, a copied or moved capture of a body used other than by name,
  a sibling-captured closure) keeps
  a full instance beside the reduced one
  (`Specializer::instantiate_nested_bodies`), and the enclosing function
  reads every binder through such a body, whose entry joins the snapshot
  with no removals. A method's unused receiver
  is a candidate too (R1012): the reduced declaration has no receiver,
  and the `MethodCall` arm becomes a `Call` of the reduced instance with
  no receiver (`Specializer::removes_receiver`), as upstream's
  `k3(q::Holder)_REMOVED_ARG()`. As upstream calls a `_REMOVED_ARG` clone
  of the generator from every direct call and keeps the original for any
  other use, the elaborator instantiates the reduced generator
  (`InstanceKey::reduced`, named by `symbol::removed_argument_symbol`,
  `fa$REMOVED$ARG$mono`, `g$REMOVED$ARG`) for a call `infer_call` binds,
  dropping the call's argument for each removed slot by the call's own
  binding against the full declaration
  (`Specializer::drop_removed_arguments`; an omitted argument's default is
  never evaluated), and the full generator, whose `removed_params` it
  ignores (`Specializer::removed_params`), for a function value, an
  export, an entry, or any other demand. Upstream makes the clone and
  rewrites the calls in the parametric program; Mojito's elaborator makes
  it per demand, since the parametric program keeps every signature and
  call (R912). The verifier reports a removed argument the body uses and
  reads the body through the same view.
  The pass is a `CallGraphPass` (R907), as upstream's walks its call graph
  callees first and rewrites each call of a generator it pruned before
  reaching the caller: a caller reads its body as that rewrite leaves it
  (`mir::without_removed_forwards`), each call, resolved method call, or
  subscript call (R971) to
  a finished callee with removed parameters dropping their arguments (the argument's register and
  expression, the `instantiated_args` entry when those align with the
  callee's declarations, a static call's receiver arguments, and the index
  operand of an accessor taking no runtime index, `__getitem_param__[i]`,
  whose index the pin spells only as the parameter attribute) and then the instructions that only computed one
  (an unread copy of a slot whose type has no destructor or that a
  callable parameter binds, a built-in
  operator that cannot trap, anything trivially dead). A built-in operator
  that can trap goes too when every read the body held of it was a dropped
  argument's or another such instruction's, as the pin's parameter argument
  is an attribute with no operation behind it (R969). So `mid[n]` that
  only calls `leaf[n]()`, or `div_fwd[n]` that only calls `leaf[10 // n]()`,
  where `leaf` ignores `n`, removes `n` too. A
  removed parameter keeps its argument only where the call omits a kept
  parameter whose default reads it, judged per call by
  `mir::supplied_parameter_arguments`, so `dmid[x]` calling `dleaf[x, 2]()`,
  where `dleaf[a, b = a]` reads only `b`, removes `x` (R970).
  The callee's removals come only from `Callees::unit_body`: a body of the
  same unit (the program's own at the program seam, never a package's, as
  upstream skips an external generator) off any cycle, so a forward around
  a recursive call stays a read, as the pin keeps the parameter. Since the
  calls are not rewritten (R912), the body records the removals it relied
  on, a callee's or a nested body's, as `MirFunction::callee_removed_params`, the verifier reads the body
  through the same view built from that snapshot, and `inline-parametric`
  merges an inlined callee's snapshot into its caller's.
  A forward to a callee's removed runtime argument is no use either
  (R1013): before it records `removed_args`, the pass reads the body with
  each direct call or resolved method call of a finished callee that
  removed arguments passing no operand for them
  (`mir::without_removed_argument_forwards`, binding by
  `mir::removed_argument_operands`, the matcher the elaborator's
  `Specializer::drop_removed_arguments` uses) and the instructions that
  only computed one erased: a whole-place or field load, a borrow, a copy
  of a slot whose type has no destructor, or a move of a parameter whose
  drop runs no code. A copy of any other value is a call of its own and
  stays, as the pin keeps `fcopy`. A removed receiver becomes a register no
  instruction defines, of the receiver's type, so the call still reads the
  struct arguments its member kept. The pass follows every callee with an
  argument (`has_parameters`), and records the callees it relied on as
  `MirFunction::callee_removed_args`, which the verifier reads through and
  the elaborator builds every instance by
  (`mir::with_removed_argument_forwards_erased`, which keeps the operands
  for the call arms to drop and the types of those it left undefined,
  shed once dropped). `inline-parametric` inlines a callee's generator
  without its removed arguments, as upstream inlines the rewritten call,
  passing no operand for them, and erases what only computed one
  (`mir::erase_removed_operands`). At the package seam a body whose finish
  reaches itself again lies on a cycle (`PackageBodies::finishing_body`):
  `PackageBodies::on_cycle` walks the bodies before drop elaboration,
  which misses a cycle a destructor call closes (R1035).
  An instance takes a struct type whose argument names a removed
  parameter with that argument erased, as upstream's generator spells it
  `_REMOVED_ARG` (R976): `Bindings::removed` holds the template's removed
  binders, which stay bound for the forwards that still evaluate them, and
  `substitute_ty`'s struct arm replaces each argument naming one by
  `CtValue::RemovedArg`, erasing whole a value argument or a type argument
  that names it outside a struct type, while a struct type argument erases
  its own (`List[Box[n]]` is `List[Box[_REMOVED_ARG]]`). So one instance
  `fb$mono` takes `Box$mono$V$REMOVED$ARG` for every `Box[n]`. The erased
  instance binds no erased parameter (`unify::bind_ty_args` records it in
  `Bindings::removed` instead, and `apply_defaults` skips it) and is
  declared with its substituted fields (`Specializer::discover_structs`),
  so it lays out as every instance agreeing on the kept arguments, upstream's
  structural layout type; one only a struct argument or a pointee names
  needs no layout and is declared only if it has one. A field or a member
  body that reads an erased
  parameter fails the instance explicitly (`Specializer::erased_layout`,
  `substitute::removed_parameter_read`). A pointer field whose pointee
  names a removed parameter or erases an argument points to `None`
  instead (`substitute::substitute_field`), as `LowerLIT` lowers every
  pointer field's pointee, so `PtrBox[_REMOVED_ARG]` and
  `List[Box[_REMOVED_ARG]]` lay out their addresses alone. Its destructor
  is the drops' to demand: the elaborator names, at each `drop.var` and
  `drop.place`, the `__deinit__` instance `enqueue` makes for the dropped
  type (`Specializer::name_lifecycle_calls`), which the struct's concrete
  member table records, names the destructor a storage destroy, a variant
  store, and an owned capture run the same way, and demands by type only
  what no instruction names (`destroyed_structs`), and names each copy's and move's constructor
  instance the same way, by type only what no copy or move names
  (`relocated_structs`); and, as for every struct, a constructor the
  calls' (R993), a `__call__` the calls' (R1009), and a copy or move
  constructor the copies' and moves'
  (`Specializer::enqueue_lifecycle_members`, R994); a copy or move of an
  erased instance or of one over an erased type
  (`Holder[Box[_REMOVED_ARG]]`) demands its constructor only when that
  member removed every erased parameter, as the generator copying or
  moving it read the member's kept parameters alone (R999). The verifier
  takes it as the type of any instance of the same template whose other
  arguments agree (`verify/types.rs`'s `erased_struct_agrees`), and the VM
  destroys, copies, and moves a value in a slot or register of an erased
  type through that type's members (`dispatch::erased_instance_value`,
  `VmBackend::{drop_value_as, clone_value_as, move_value_as}`), as
  upstream's lifecycle calls are static, rather than by the value's own
  instance name, which a copy or move keeps.
- **Two seams, once per body.** `CompiledProgram::elaborated_mir` runs
  `pre_elaborate_program` over the program's own bodies after
  `elaborate_drops_program` (the `pre_elaboration` `--timings` span), and an
  imported package's finishing hook (`finish_package_body`, a
  `mir::text::package::BodyFinish`) runs it after `elaborate_drops_function`
  as each body is first materialized. The pipeline runs once per generator,
  never per instance. At the program seam a call-graph pass runs over the
  program's bodies in the bottom-up order of the strongly connected
  components of the calls it follows (`run_call_graph_pass`), each callee
  rewritten before its callers; a callee the program does not define comes
  from the imported package (`pre_elaborate_program`'s `package`), finished
  on first use. The finishing hook is given the package's bodies, so a
  package body's callees come finished from the same package; a package
  body never calls a program body, so no cycle crosses the seam. Finishing
  a package body runs every call-graph pass, so a package callee's cycle
  check (`PackageBodies::on_cycle`, memoized by row) follows the calls any
  `PIPELINE` call-graph pass follows: a cycle mixing two passes' edges
  would otherwise re-enter a body still being finished.
- **Where it does not run.** A package build serializes pre-drop MIR, so it
  never reaches the program seam; its bodies get the body and call-graph
  passes at import.
  The package seam runs no program pass: a package body is finished only when
  the elaborator first reaches it, so no finished body is dead. A
  serialized artifact is already past the seam and is not run again.
  `__toplevel__` is skipped, as ASAP drops skip it, so its slots keep the
  values `bindings()` reports.
- **Verification.** In debug builds each body is verified after every body
  or call-graph pass that changed it (`run_pipeline`), each finding naming
  the pass, into `invariant_errors` or
  the package body's verdict (`PackageBodies::findings`), which the
  elaborator refuses when it materializes the body. The post-drop verify
  after the seam runs in every build.
- **Pass tests.** `pass_named` looks a pass up by name, as MLIR's pass
  registry does: every `PIPELINE` pass, plus `ELABORATE_DROPS`
  (`elaborate-drops`), drop elaboration run on request, and
  `LOWER_CONTROL_FLOW` (`lower-control-flow`), the transitional lowering of
  structured region ops to blocks, which MIR lowering calls at the end of
  each body. `analysis::opt`,
  the `mlir-opt` analogue behind `mojito opt [--pass NAME]… [FILE]`, loads a
  MIR artifact, runs the named passes in order (a program pass over the
  program, a body pass over every body but `__toplevel__`, a call-graph pass
  over the same bodies callees first, with no package), and verifies the
  whole program after each pass in every build. `tests/mir_pass_test.rs` pins each case under
  `tests/mir_passes/<pass>/` by its `functions:` section
  (`mir::text::disassemble_functions`), so a format bump breaks no expected
  file.
- **The switch.** `MOJITO_PRE_ELABORATION=0` turns the pipeline off, and
  `Compiler::with_pre_elaboration(bool)` sets it whatever the environment
  says. The bundled package is imported once per setting
  (`bundled_std_package(pre_elaboration)`), since a finished body is cached
  with the hook that finished it; both share one cache file, which holds
  pre-drop bodies.
- **The differential.** The `pre_elaboration::*` corpus group
  (`tests/corpus_test.rs`) runs every executable fixture (`assets/ok`,
  `assets/extensions/ok`, `assets/runtime_error`) with the pipeline on and
  off, in process or, for an `input()` fixture, through the binary with
  piped stdin. It requires the same output, the same error text, and no
  verifier finding in either run; a finding in a package body the program
  reaches fails its elaboration. The `quick` profile excludes the corpus binary, so the group
  runs in the overnight gate.

### Post-Elaboration Pipeline

Upstream nests a per-function `'kgen.func'` pipeline after
`ElaborateGenerators`: the pinned compiler's `--mlir-timing` report lists
`SROA`, `Mem2Reg`, `SCCP`, `Canonicalizer`, and `CSEPass` on each concrete
function. Mojito's is `analysis::POST_ELABORATION_PIPELINE`
(`crates/mojito-analysis/src/analysis/post_elaboration.rs`, R840):
`lower-arg-conventions`, `mem-2-reg`, `sccp`, `canonicalize`, then `cse`,
the pre-elaboration pipeline's own body passes but the first, then MLIR's
CSE, without the upstream pass Mojito does not have (`SROA`). Upstream's first
optimization pipeline after elaboration runs `LowerArgConventions` and then
`Mem2Reg` on every function: a register-passable `read` or owned argument
is stored into a callee-side temporary, which `Mem2Reg` promotes, while a
`mut` one is not. `lower-arg-conventions` (`analysis::LOWER_ARG_CONVENTIONS`,
`analysis/mem2reg.rs`, R787) is that callee half: each `read` or owned
parameter of a trivially register-passable type that `mem-2-reg` would
promote, were it stored at the entry, gets a fresh local every mention of
it names, stored from one `var.use` of the parameter at the entry, so the
call ABI's slot is read once and `mem-2-reg` promotes the local. A body
whose entry block is a jump's target is left alone. `mem-2-reg` then
promotes the instance's locals too, whose types the instance closed. It folds what only
an instance knows: the elaborator answers a parameter read
(`Const::Param`) with an ordinary scalar `const`, so the operators over it
fold and a `branch` it decides becomes a `jump`, and a copy whose type
substitution made trivial is forwarded. A dispatched `x.copy()` whose
instance makes the receiver a built-in arrives as such a copy: the
elaborator writes it as `value.copy`
(`mono/substitute.rs:copy_builtin_receivers`), as `Copyable.copy`'s body
`Self(copy=self)` is at that `Self`. A receiver read through a `ref`
binding (`for value in values: value.copy()`) folds the same way: the
instance keeps the substituted generic register type, which types a
whole-root load of a reference root by its referent, as lowering and both
backends read it. An instance's register and slot types are the generic
MIR's types with its bindings substituted, never re-derived from one
another, as upstream's elaborator substitutes a generator body's types; the
concrete verifier rejects a reference slot read as its handle.

- **Dead symbols.** Upstream runs `EliminateDeadSymbols` straight out of
  the elaborator and again after the function pipeline (`Pipeline.cpp`).
  So does Mojito: `SpecializedProgram::pruned` runs
  `analysis::eliminate_dead_concrete_symbols` over the concrete program on
  each side of the pipeline, rooted at the entry roots, the requested
  entries' instances, and the compilation's exports
  (`Compiler::with_exports`). Every concrete call names its callee, so a
  constructor or overload nothing names goes; a struct's member table
  names the constructors the backends run with no call. The
  pre-elaboration run keeps every constructor of a live struct and every
  member of a live overload family instead, since the elaborator resolves
  a generic body's calls by family and by type. The
  instances the elaborator made only to run a compile-time application on
  the VM, such as the `paramfor_*` stubs, `range` iterator methods, and
  `$comptime$` thunks, leave the concrete program. Compile-time
  evaluation runs fragments built apart (`Specializer::fragments`), so the
  pruning changes no evaluation. The elaboration counts keep only the
  instances that remain.

- **Duplicate functions.** Before that first dead-symbol run, upstream's
  `EliminateDuplicateFunctions` keeps one of each set of identical
  functions, and so does `analysis::eliminate_duplicate_functions`
  (`analysis/duplicate_functions.rs`, R990), the first of
  `analysis::ELABORATED_SYMBOL_PASSES`. Functions are identical when they
  share a `mir::text::package::function_identity`: the body's encoding,
  with each reference to the function itself spelled alike, its
  registers' source locations, and its declaration's encoding. The first
  of each set stays and every other's references are pointed at it with
  `mir::replace_symbol_uses`, MLIR's `replaceAllSymbolUses` over the
  references an instruction holds itself; that can make callers
  identical, so the pass repeats until no set merges. A function stays
  under its own name when anything else reaches it: an entry root, an
  export, or a use `mir::SymbolTable` finds that the rewrite does not
  reach, such as an implicit callee or a symbol a type names, found by
  counting one against the other. A struct's member table entry is no
  such use: as upstream's `replaceAllSymbolUses` redirects a witness
  table, the pass points an entry naming a merged member at its survivor
  (R1006), so `Sparrow.fly` and `Sparrow.eat`, both `pass`, merge.
  Upstream's call carries a compile-time argument as an attribute, so an instance holds no register for it; the elaborator
  matches that by erasing, with each parameter argument slot it drops,
  the constant the slot's register was read into once nothing else reads
  it (`erase_dropped_argument_reads` in `native/mono/specializer.rs`,
  R991), so callers apart only by that read merge with their callees.

- **CSE.** `cse` (`analysis::CSE`, `analysis/cse.rs`, R845) is MLIR's
  `CSE`: it walks each block list's dominator tree (Cooper, Harvey, and
  Kennedy's iteration over `mir::reverse_postorder`) with a scoped table,
  and an instruction free of effects that a dominating one matches
  (mnemonic, attributes, operands, result type) is erased, its result's
  reads renamed. Free of effects is narrower than
  `MirInstr::would_be_trivially_dead`: a constant, a literal
  materialization, a size or type-name query, a parameter list's address,
  a field or variant-tag read of a value register, or a SIMD construction,
  cast, or shuffle; a read of storage or through a reference is a read
  effect MLIR merges only with no write between, which `cse` does not do
  yet (R929). Only a result held whole in one register is merged, since a
  reader may consume a value held in storage, and each merged result and
  operand has one definition the walk has passed on the dominating path,
  since MIR does not verify that a definition dominates its reads. The
  Pliron backend lowers each block list in `lowering_order` (reverse
  postorder, then the unreached blocks), so a definition `cse` reuses in
  a later-numbered block is lowered before its reads.
- **The seam.** `analysis::post_elaborate_program` runs each pass over
  every concrete function but `__toplevel__`, on the driver thread after
  `native::mono` returns (timing counters are thread-local, and the
  elaborator runs on its own deep-stack thread). It runs under the
  `post_elaboration` `--timings` span, nested in `elaborate`, so its
  counters read `elaborate.post_elaboration.sccp.folded_constants` and so
  on. `native::mono` does not depend on `mojito-analysis`: the driver hands
  the pipeline to `SpecializedProgram::rewritten`, which knows no pass.
- **Re-admission and recount.** Each `BodyPass` returns whether it changed
  the body, `false` being MLIR's `markAllAnalysesPreserved`, and
  `post_elaborate_program` returns the indices of the functions some pass
  changed (counter `elaborate.post_elaboration.unchanged_functions`).
  `SpecializedProgram::rewritten` runs the rewrite through
  `ConcreteMir::rewrite_bodies`, which hands it the function list as a
  slice and the declarations shared, so neither can change, and re-admits
  the program by verifying only the changed bodies
  (`verify::verify_concrete_function_in`); a debug build re-verifies the
  whole program. It then recounts the `ElaborationCounts`
  (`ElaborationCounts::recount`, from each generator's
  `GeneratorCount::instance_names`), so `concrete_instructions` measures
  what the backends receive. No body is verified after each pass.
- **Callers.** `CompiledProgram::concrete_mir` (behind the switch),
  `artifact::run_artifact` (`exec`, always), and
  `mojito_pliron::compile_mir` (a scalar entry requested by name, always).
  The compile-time applications the elaborator runs on the VM read the
  elaborator's unfolded fragments.
- **The switch.** `MOJITO_PRE_ELABORATION=0` and
  `Compiler::with_pre_elaboration(false)` turn this pipeline off along with
  the pre-elaboration one, so the `pre_elaboration::*` differential covers
  both.

## Stage 8: Register VM

Module:

```rust
crates/mojito-vm/src/backend/vm.rs
```

The register VM executes concrete MIR and nothing else: production hands
it `Backend::run_concrete`, in which every call names an instance, and
compile-time evaluation hands it a concrete fragment
(`VmBackend::call_concrete`). A parametric instruction, terminator, or
constant it meets is a compiler invariant error, never a runtime branch. It
is structured rather than byte-addressable:

- registers hold rich `runtime::Value`s
- variables are frame slots
- public collections are ordinary nominal struct values; their storage uses the
  same pointer arena and struct fields available to user code
- private heterogeneous packs, CTFE-only lists, strings, errors, variants,
  slices, and SIMD values retain dedicated runtime carriers
- field and index operations work through high-level value navigation
- calls allocate a new VM frame

Frames are owned by an explicit VM stack and have monotonic identities. Direct
user-function calls push a frame plus a return/write-back continuation; returns
pop and resume the caller in the iterative dispatcher. This makes ordinary deep
source recursion independent of the Rust call stack. The frame shape is:

```text
id: FrameId
function/block/instruction cursor
registers: Vec<Value>
variables: Vec<Value>
return continuation
```

`regs` are temporaries. `vars` are source variables, parameters, and compiler
synthetic locals.

### Program Metadata

The VM builds a `Prog` containing:

- lowered MIR
- struct definitions and field layouts
- method mutability information
- function signatures
- default arguments
- value-parameter declarations
- signature-mangled overload definitions and fallback lookup for unique arity
  protocol calls

The checked entry point is the production path. It normalizes declaration facts
into `MirDeclarations`: struct field layouts and callable parameters use checked
`Ty`, defaults use `CheckedConst`, and overload names come from `CheckedProgram`.
The VM builds compact registries from this metadata rather than rescanning AST
annotations, reevaluating default expressions, or recomputing overload choices.

### Function Calls

Calling a function:

1. resolves the function index
2. matches arguments to parameters
3. coerces arguments to parameter types
4. creates a new frame
5. writes arguments into parameter variable slots

For a homogeneous `var **kwargs: T` collector, the checker leaves explicit parameter
binding unchanged and type-checks unmatched keyword values as `T`. The same
logic participates in generic inference and in free, instance, static, and
bounded-trait method selection. The ABI preserves unmatched pairs in call-site
order. The VM constructs the implicitly linked, self-hosted `StringDict[T]`
directly in the collector's callee slot; it is an owned mutable local and never
participates in caller write-back. The native caller builds the same
instance in fresh storage through its empty constructor and one `__setitem__`
per collected keyword, then passes it owned. `callee(**kwargs^)` consumes that dictionary,
moves its ordered entries back into the shared binder, and retains ordinary
duplicate/missing-keyword diagnostics and the checker-selected effect contract.
6. binds value parameters into frame locals
7. runs the callee's block loop
8. returns the result and, where needed, final variable slots for write-back

`mut` and explicitly mutable `ref[origin]` parameters receive handles to the
simple caller places retained in MIR. Keyword places are resolved after argument
binding by the selected parameter name, so reordering and ordinary defaults do
not lose identity; a value synthesized by `**kwargs^` has no writable source
place. Inside a structured region, a temporary caller-frame mirror makes these
the same frame/slot handles used by continuation-driven calls and is committed
on both normal and raising outcomes. Bare `ref` remains parametrically mutable
and may not write in an unspecialized body.

Overloaded function calls arrive at the VM already resolved to a lowered
signature name. For constructor overloads, a direct resolved callee such as
`Box.__init__$ov$String` still enters the constructor path: the VM creates the
uninitialized `self` skeleton, binds the remaining arguments through the same
positional/default/keyword matcher used by ordinary calls, invokes the selected
`__init__`, and returns the initialized struct. Internal dunder/protocol paths that do not have a source call
span can still ask for a unique overload by source name and arity; this is a
fallback for compiler-generated calls, not the general overload-ranking engine.

### Method Calls

Method calls are normal function calls with a receiver convention:

- `self` is parameter slot 0
- ordinary arguments use the same positional/keyword/default/variadic slot
  matcher as free functions before `self` is prepended
- `mut self` writes the final receiver back to the caller place
- ordinary `mut` and explicitly mutable `ref[origin]` method parameters also
  bind through retained positional or keyword caller places
- nominal collection mutators commit through a reference-aware receiver place,
  including pointer-backed self-hosted `List` fields

Method-dispatched nominal `Index`, `Slice`, `MultiIndex`, and `MultiSet` enter
this same selected-method dispatcher and argument binder. They never choose a dunder by
runtime name or arity: the retained `MirSubscriptCall` supplies the lowered
target, value-parameter arguments, typed raising contract, caller places, and
reference-result metadata. The VM only materializes slice descriptors, invokes
that contract, performs selected write-back, and returns or propagates its
result. The call-less `Slice.indices()` Tuple-storage bridge described above is
the sole nominally typed exception.

### Moves At Runtime

Static ownership analysis should reject invalid moves before execution. The VM
still models move effects:

- moving a variable transfers the value out of the source slot
- the source slot becomes moved/empty
- moving a field leaves a moved marker in that field
- using a moved slot at runtime is a loud error, not silent behavior

This makes the VM a useful backstop and executable model for ownership semantics.

### DropVar At Runtime

`DropVar` removes the value from a variable slot and recursively destroys it.

Dropping is observably a no-op for scalars and destructor-less leaf values. For
a struct, it calls the destructor the drop names (which consumes the
receiver's fields); a drop naming none drops the fields. A
destructor-less struct still recursively destroys aggregate fields; elements
inside compiler-private tuple storage are visited left-to-right, and those
of an owning positional collector last to first. Moved-out fields are skipped so partial moves do not double-drop.

### Exceptions And Non-Normal Flow

`raise` propagates the original runtime value as a `Raised` error until a `Try`
catches it. This preserves fields on user-defined typed error structs; string
raise shorthand is normalized to the builtin `Error` value.

Inside try sub-regions, the VM uses a control-flow enum conceptually like:

```rust
Normal
Return(Value)
Jump(MirBlockId)
```

This lets `return`, `break`, and `continue` cross a `try` boundary while still
running cleanup and `finally`.

The rule is:

- body raise goes to `except`, if present
- `else` runs only after normal body completion
- `finally` always runs
- non-normal flow from `finally` overrides the pending body/handler/else outcome

## Runtime Values And Builtins

Module:

```rust
crates/mojito-vm/src/runtime.rs
```

The VM operates on `runtime::Value`, the shared representation for supported
runtime values:

- integers, unsigned integers, floats, booleans
- strings
- `None`
- structs, including public `List`, `Set`, `Dict`, `Range`, and `Tuple`
- CTFE-only `ComptimeList` values
- compiler-private heterogeneous pack tuples
- variants (checked alternative list, active tag, and payload)
- slice descriptors (contiguous, strided, or general, with optional bounds)
- SIMD-like lane vectors
- errors
- moved/tombstone markers

Runtime helpers implement:

- arithmetic and comparison
- prefix operators
- coercion and numeric conversion
- string display
- list methods
- SIMD construction and lane access
- builtin functions such as `print`, `len`, `range`, numeric conversions,
  `abs`, `min`, `max`, and `round`

`size_of[T]()` is deliberately not a VM-value helper. Checking records `T`, MIR
retains it in `SizeOf`, verification requires it to have a native layout, and
both the VM and native lowering ask the shared `native::layout::LayoutCx` for
the byte count. This keeps target layout below the checked-MIR waist without
duplicating layout policy in either backend.

`external_call["callee", ReturnType, num_fixed_args=n](args...)` is the one
host boundary below the stdlib: a builtin over the closed libc callee table
`mojito_types::ffi::CALLEES`. The checker (`checker/ffi_calls.rs`) accepts
only an allowlisted callee (any other name is a contextual error), checks
each argument and the declared return type against the C prototype, and
types the call as never raising; MIR carries it as an ordinary named `Call`
whose instantiated arguments are upstream's parameters in order (the callee
string, the return type, the fixed-argument count), every backend reads the
callee from the first of them, and the result type from the destination
register. The VM
(`backend/vm/libc.rs`) executes each callee on Rust's standard library with
libc's observable contract — return value, an `errno` slot behind
`__errno_location`, bytes written through pointer arguments (`readdir`
produces glibc `dirent` images, `__xstat` fills the caller's `_c_stat` by
field name) — over a descriptor table where fd 1 appends to the captured
stdout and `setenv`/`unsetenv` write a per-VM overlay; the native backend
declares the C function on demand and calls it. No compiler-private
`_mojito_*` crossing exists for I/O. `print(sep=, end=, flush=, file=)` is the
same builtin with keywords: the VM joins the cells and writes them to the
captured stdout or, for `file=`, through its descriptor table; the native
backend routes every piece of such a call through libc `write` on the
`RawFileDescriptor`'s value instead of `mjrt_write_stdout`.

The `with` statement is a checker desugar (`checker/with_stmt.rs`), not a
lowering construct: the manager's declared `__enter__`/`__exit__` methods
select one of a few ordinary statement shapes (`VarDecl`, `Try`, `If`,
`Raise`, `Expr`), the checker checks that desugar in a block scope, and after
the last transfer round it splices the desugar into the checked tree in the
statement's place. HIR, MIR, ownership analysis, and the backends therefore
never see a `With` node. Each synthesized node's identity is derived from the
statement's (`SyntaxId::derived`), so every rebuild of one statement's desugar
names the same occurrences. The one compiler-private spelling it emits is
`_mojito_keep_alive(name)`, a statement MIR lowers to the existing
`KeepAlive` liveness anchor (resolved through the argument's checked binding,
so a later block rebinding the same `as` name anchors its own slot): it keeps
the manager — or a consuming `__enter__`'s result standing in for it — alive
to the end of the block without a copy or a move, while an `as` binding is an
ordinary local destroyed at its last use, as the pinned Mojo does. The unbound
result of a non-consuming `__enter__` is not anchored: it is a discarded
temporary destroyed before the body runs.

A call through a trait bound that leaves out a requirement's default runs
the requirement's default, as in current Mojo, even though an instance clone
reaches the witness nominally. The check records each such call's omitted
defaults that some witness declares otherwise (`checker/bound_defaults.rs`),
keyed by the call's origin, and spells them as literal keyword arguments of
the call and of every clone of it; a pass that finds a new one is followed by
another over the spelled tree, and the template catalog carries them so a
later pass binds them before it starts. The spelled
nodes' identities are derived from the call's, so a clone's traces to the
template's. Every checked signature holds a default as an attribute
(`SignatureDefault`), as the lowered declaration does: a literal constant,
an evaluated default, or, for a trait requirement only, a parameter
expression. The trait's declaration makes each requirement default one
(`requirement_default` in `checker/traits.rs`): a module constant, or a
compile-time `Int` expression over them, becomes the constant it names in
the trait's scope, and a default reading the method's own value parameters
becomes a `ParamExpr` over them with its module constants folded. The call
evaluates that expression with its solved binding of the parameters
(explicit, defaulted, or inferred) and spells the value in the caller's
scope, which means the same in each clone; any other default is rejected.

Keeping value-level behavior in `runtime` prevents the VM from baking every
operation directly into the backend. The VM should be a consumer of checked MIR
plus runtime primitives, not a second checker.

## Unsupported Constructs

Unsupported constructs should be explicit.

Preferred behavior:

- parser accepts Mojo-like syntax when possible
- checker rejects unsupported semantics early when it can
- MIR may contain `MirInstr::Unsupported` for late-discovered backend gaps
- VM reports a clean `RuntimeError::Unsupported`
- tests assert unsupported behavior instead of allowing panics

This is important because mojito parses more syntax than it fully implements.
A clean unsupported error is part of the architecture.

## Fixture And Test Relationship

The architecture is reflected in test layout:

- parser tests check AST shape
- checker tests check type and semantic acceptance/rejection
- HIR tests check CFG shape
- MIR tests check lowering shape
- ownership tests check move analysis
- drops tests check ASAP destruction
- VM tests check execution
- `assets/` fixtures exercise whole-pipeline behavior

Accepted `.mojo` programs belong in:

```text
assets/ok/
```

Ownership-specific fixtures belong in:

```text
assets/ownership_ok/
assets/ownership_error/
```

The asset harness turns examples into executable documentation. A feature is more
real when it has a fixture.

## Architectural Boundaries

### Checker vs MIR Analysis

The checker should answer questions that are local to declarations,
expressions, types, and calls.

MIR analysis should answer questions that require control flow:

- has this value been moved on all paths?
- has it been maybe-moved on one path?
- where is the last use?
- where should destruction occur?
- which branch edge needs a cleanup block?

### HIR vs MIR

HIR owns statement-level control flow while expressions remain nested.

MIR owns expression flattening, register allocation, places, and instruction
semantics.

If a feature needs to know the order of subexpression evaluation, it belongs in
MIR or later. If it needs only branch/loop shape, HIR is the right layer.

### MIR vs VM

MIR should preserve enough semantic facts that the VM does not need to infer
language rules from source syntax.

The VM may still hold runtime metadata such as struct field layouts and function
signatures, but the direction should be toward checked declarations and MIR
metadata becoming the source of truth.

### Runtime vs Backend

The runtime module owns value operations. The VM owns execution order, frame
management, calls, jumps, drops, and exception flow.

This separation makes it possible to add another backend later without
reimplementing every scalar/list/string/SIMD rule from scratch.

### Textual MIR/VM assembly boundary

The normative version-1 grammar, compatibility policy, canonical ordering, and
serialization inventory live in [`mir-text-format.md`](mir-text-format.md).
`mir::text` owns the matching version constants, closed mnemonic vocabulary,
and the canonical `disassemble` entry point. The printer rejects MIR invariant
or verifier findings before emitting any text, serializes checked semantic
metadata directly without AST or `Debug` reconstruction, and normalizes
unordered tables through sorted borrowed views. The later parser must share
this vocabulary.

`mir::text::parse_artifact` owns the inverse syntax boundary. It validates UTF-8
bytes, parses a Mojo-independent spanned schema tree, and decodes the complete
schema — every instruction, terminator, type, origin, and declaration-metadata
form the printer emits — while retaining an `ArtifactSourceMap` separate from
serialized Mojo source locations. Nested `try` regions decode through their
own dense per-region block namespaces and are deliberately absent from the
source map: the enclosing instruction path brackets them, and the canonical
verifier resolves only function-level block paths. Parsing does not invoke semantic MIR verification:
`mir::text::verify_artifact` composes the canonical `mir::verify` pass on the
returned `ParsedArtifact` and maps each finding to its assembly span (block,
then function, then artifact-root precision) by parsing the verifier's
canonical `MIR function '<name>' [block <n>]` message prefixes — the one
sanctioned consumer of that spelling. `load_artifact` bundles parse-then-verify
as the loading gate artifact execution sits behind; verification policy itself
never moves out of `mir::verify`. `artifact::run_artifact` composes that gate
with the elaborator and `Backend::run_concrete` (the CLI `exec` subcommand's
engine): the artifact is drop-elaborated MIR, which may be generic, and the
loaded program is elaborated to concrete MIR from its entry roots exactly as
a compiled source program is. It is not drop-elaborated or
ownership-analyzed again. The composition lives in
`src/artifact.rs`, beside rather than inside `Compiler`: the artifact path
deliberately bypasses the source pipeline, and `mir::text` cannot host it
without layering onto `backend`.

The compiler exposes a textual, flattened, versioned serialization of
verified MIR and the metadata needed to execute it. The format is built for
determinism and tooling rather than human reading — even trivial programs
serialize to long artifacts. It must support:

- deterministic printing suitable for review and golden tests (implemented)
- parsing with source-located diagnostics (implemented for the full schema)
- structural and semantic verification before execution (implemented)
- lossless print/parse/print round trips (implemented; enforced byte-for-byte
  per executable corpus fixture by the `roundtrip::*` group of
  `tests/corpus_test.rs`)
- disassembly of verified in-memory programs (implemented)
- execution by the register VM without reconstructing source AST semantics
  (implemented: `artifact::run_artifact` composes `load_artifact` with
  `native::mono` and `Backend::run_concrete`. The serialized program is
  already drop-elaborated — re-running `elaborate_drops_program` is unsound
  because drop elaboration is not idempotent, and the pre-drop ownership
  analysis has no meaning post-drop; verify-at-load is the consumer gate)
- consumption by future native backends (Pliron and Cranelift first)

`mojito emit-mir [FILE]` exposes the producer boundary for files or standard
input. It prints only the canonical post-drop artifact accepted by `mojito
exec`, so the two commands compose directly in a pipe. Every shared runnable
conformance case executes both from the cached compiler MIR and after textual
serialization/loading; output and observable bindings must agree.

This is a Mojito format, not a generic interchange standard. The in-memory MIR
remains authoritative; textual assembly is its stable inspection and artifact
boundary. A compact binary encoding may later share the same schema.

## Current And Future Pressure Points

The main pressure points are:

- CTFE function-body execution uses restricted MIR/VM execution, while nested
  generic requests return to the same structural specialization worklist
- compile-time declaration generation is deliberately structural: reflection
  selects parsed declarations through `comptime` control flow rather than a
  string-to-AST macro channel
- ABI-sensitive reflection such as byte offsets belongs to the future native
  backends; VM reflection exposes semantic field indexes and checked projections
- MIR is fully register-typed and semantically verified; checked capture and
  binder facts cross HIR without source-name/span reconstruction. The remaining
  compatibility boundaries are name-based callee fallbacks kept only for the
  unchecked phase-test path and nominal callable-conformance facts in
  `MirDeclarations`
- the backend-ready MIR checkpoint is closed: abstract erased-dispatch
  requirements live in typed call-local contracts, callable-value requirements
  live in their stored `TyKind::Func`/`TyKind::GenericFunc` contracts, variadic
  conventions are explicit declaration fields, and reference loans are checked
  against their executable capability permission and canonical interior owner
- source modules and packages are flattened after lexical namespace resolution;
  compiled `.mojoc` artifacts remain future distribution tooling
- trait support is intentionally incomplete; in particular, associated
  compile-time types are monomorphic, so origin-parameterized iterator families
  must cross the checked/MIR boundary before its textual schema freezes
- abstract trait-dispatch subscripts are verified from their complete
  checker-retained argument/result requirement and retargeted to a concrete
  method only at execution
- exception modeling is structured, not a fully general unwind-edge MIR
- nested-function and capture support should match Mojo's non-escaping patterns
  without growing into a general escaping-closure system
- more library types can migrate from runtime/compiler support into self-hosted
  modules as the language subset gets stronger
- diagnostics should continue moving from "correct" to "pleasant"

## Mental Model

Read the compiler from the middle outward:

1. MIR is the contract.
2. HIR exists to make control flow explicit before MIR.
3. Module linking assembles imported declarations into one program.
4. Comptime elaboration erases compile-time control and materializes constants
   before runtime checking.
5. The checker prevents unsupported or ill-typed programs from reaching MIR.
6. Analysis proves ownership and inserts destruction.
7. The VM executes what MIR says.

That is the core architecture of mojito after parsing.

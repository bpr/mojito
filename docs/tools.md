# Formatting and linting

Mojito should grow a formatter and a linter as one source-tooling subsystem,
not as two unrelated commands. The formatter should feel like `mojo format`;
the linter should provide Ruff-like diagnostics, selection, suppression, and
safe fixes. Both should be fast enough to run on save and across a repository.

The most important design choice is to share the expensive front end. A source
file should be discovered, read, lexed, and parsed once. That one lossless
snapshot should serve formatting, lint rules, fixes, diagnostics, and future
editor support. Syntax-only tooling must not link modules, elaborate comptime
code, build HIR or MIR, or initialize a backend.

## What to borrow from Ruff

Ruff's useful ideas form a coherent architecture rather than a bag of small
optimizations:

- Its formatter separates language-specific AST formatting from a generic
  document IR and printer. Groups, indentation, soft and hard line breaks, and
  conditional content describe layout without repeatedly constructing trial
  strings. Comments are attached to syntax nodes as leading, trailing, or
  dangling trivia, and the implementation checks that every comment was
  emitted.
- The linter parses once and gates work by the enabled rules. Token checks, AST
  traversal, import analysis, semantic analysis, and filesystem checks only run
  when at least one selected rule needs them. A shared traversal dispatches to
  interested rules instead of walking the tree once per rule.
- Diagnostics and fixes are independent of individual rules. Edits have an
  applicability level, are checked centrally for conflicts, and are applied in
  bounded iterations with cycle detection.
- Files are processed independently in parallel, but results are sorted before
  presentation. Configuration is resolved before the hot per-file loop.
- Cache lookup happens before reading and parsing when metadata makes that safe.
  Cache keys include the effective settings and tool version, and cache writes
  are atomic.
- Correctness is treated as a performance feature: formatter idempotence,
  comment coverage, parser round trips, fuzzing, and compatibility tests prevent
  fast corruption.

These conclusions come from Ruff's
[contributor architecture](https://github.com/astral-sh/ruff/blob/main/CONTRIBUTING.md),
[formatter internals](https://github.com/astral-sh/ruff/blob/main/crates/ruff_python_formatter/CONTRIBUTING.md),
[formatter documentation](https://docs.astral.sh/ruff/formatter/), and
[configuration model](https://docs.astral.sh/ruff/configuration/). Astral's
accounts of its [hand-written parser](https://astral.sh/blog/ruff-v0.4.0) and
[lexer optimization](https://astral.sh/blog/ruff-v0.0.281) reinforce a useful
rule for Mojito: measure whole-tool latency, keep representations compact, and
avoid allocations and passes before reaching for exotic algorithms.

We should copy the architecture, not Python-specific policy. Mojo syntax,
comments, ownership, comptime behavior, and the official tool's output remain
the authority. The current [`mojo format` interface](https://docs.modular.com/mojo/cli/format/)
also gives us the initial user-facing conventions: source operands, a quiet
mode, and an 80-column default with a line-length override.

## User experience

The first formatter command should be:

```text
mojito format [--check | --diff] [-l N] [--quiet] [PATH ...]
```

With no path it formats the current project; `-` means standard input. Normal
mode rewrites files atomically, preserving permissions and the existing final
newline convention. `--check` changes nothing and exits unsuccessfully when a
file differs. `--diff` prints deterministic unified diffs. Later additions can
include `--range`, `--stdin-filename`, `--preview`, and `--no-cache`. The style
should intentionally have few options: line length is configuration; basic
spacing and layout are not.

The linter command should be separate from the existing semantic `check`
command:

```text
mojito lint [--fix] [--unsafe-fixes] [--select RULES]
            [--ignore RULES] [--output-format text|json] [PATH ...]
```

Diagnostics need a stable code and name, a primary source range, optional
secondary labels and notes, and fix availability. Human output should be terse
and editor-friendly; JSON provides a stable machine interface. Results must be
ordered by normalized path, byte offset, and rule code regardless of thread
scheduling. Safe fixes are the default for `--fix`; unsafe or behavior-changing
fixes require explicit opt-in.

Both commands should use one `mojito.toml` configuration discovered by walking
from each source file toward the project root. Command-line values override the
file. Shared settings cover include/exclude patterns, cache location, preview
mode, and source roots; `[format]` and `[lint]` own tool-specific settings.
Configuration discovery and merging must be documented and deterministic.

## A lossless source snapshot

Mojito's compiler lexer currently discards whitespace and comments, while AST
nodes retain spans but not all original spelling. Reconstructing source from the
AST would therefore lose information and force the linter and formatter to do
their own scans. The first implementation milestone is a lossless, immutable
`SourceDocument` containing:

- the original source, preferably shared with `Arc<str>`;
- compiler tokens with byte ranges;
- whitespace, newline, and comment trivia ranges into the original source;
- a lazily built line index for byte-to-line/column conversion;
- the parsed AST and parse diagnostics; and
- the source identity used in diagnostics and caches.

The original text remains the owner of token spelling. String and numeric
literals should initially be printed from their source slices unless a
deliberate normalization is proven compatible with current Mojo. The compiler's
existing token stream and parser behavior must remain unchanged; a lossless lex
entry point can collect trivia alongside normal tokens without injecting trivia
tokens into the parser.

This snapshot is the boundary shared by the tools. Parsing errors stop normal
formatting and AST lint rules without modifying a file, while text- or
token-level diagnostics that are sound on malformed input may still run.

## Formatter design

Formatting has four stages:

1. Build the lossless source snapshot once.
2. Attach comments to the nearest syntax node as leading, trailing, or dangling
   trivia, with explicit handling for end-of-line comments and empty constructs.
3. Walk the AST and emit a document IR containing text, groups, indentation,
   hard and soft line breaks, line suffixes, conditional content, verbatim
   regions, and source-position markers.
4. Print the document against the configured line width in one buffered pass.

The printer should be independent of Mojo syntax but can initially live inside
the formatter crate. That keeps the abstraction honest without creating a
general-purpose public crate prematurely. Parentheses and operator precedence
must be centralized rather than reimplemented by expression-formatting rules.
Every AST formatter should destructure its node exhaustively so a new field or
variant causes a compile failure instead of being silently omitted.

Comments are a hard correctness boundary. Formatting should fail in debug and
test builds if any comment was neither printed nor intentionally covered by a
verbatim region. Support `# fmt: off`, `# fmt: on`, and statement-scoped
`# fmt: skip` by copying exact source slices; directives inside strings do not
count. Range formatting can follow later by expanding a requested range to safe
enclosing statements and using source-position markers to recover the produced
range.

The style should be established from current Mojo examples and differential
probes against a pinned Mojo release. Where behavior is undocumented, Mojito
should choose and document a deterministic rule rather than chasing incidental
output. Preview-gated rules allow improvements without destabilizing the normal
formatter.

## Linter design

Each rule declares metadata and the minimum analysis it needs. The rule registry
then computes an analysis plan for the selected rules:

```text
path/text -> tokens -> AST -> scopes/imports -> checked semantics -> project
```

The common case must stop at the earliest sufficient layer. Most editor linting
should require only the lossless parse and a lightweight scope model. Rules that
truly depend on types, ownership, or module resolution may opt into checked
compiler facts, but selecting no such rule must not invoke the production
compiler pipeline.

Initial default rules should be few, high-confidence, and non-overlapping with
the formatter. Good first candidates are unused imports, unused bindings,
unreachable statements, redundant `pass`, and self-assignment, after each is
checked against current Mojo semantics. Formatting preferences should not be
lint rules. Stable numeric codes and descriptive names should be assigned before
release, with new or potentially noisy rules gated behind `preview`.

Inline suppression should use a Mojo comment convention such as
`# noqa: M001, M003`, plus per-file ignores in configuration. Suppression
parsing is one token-level pass shared by all rules. An unused-suppression rule
can eventually keep these comments from accumulating.

All fixes become text edits over byte ranges. A central planner sorts edits,
rejects overlaps, respects safe/unsafe applicability, and isolates fixes that
cannot be composed. After applying a batch, Mojito reparses and reruns the
affected rules. Iteration is capped and repeated source hashes terminate cycles.
No rule writes files directly.

## Keeping it fast

Performance requirements should shape the interfaces from the start:

- discover files and effective configurations once, then process independent
  files with a bounded Rayon pool;
- retain a single source buffer and refer to it by byte ranges instead of
  cloning lexemes, comments, paths, or diagnostics;
- parse once per file and lazily create line indexes, scope data, and checked
  facts;
- precompute a compact rule-requirement bitset and skip whole passes when no
  enabled rule uses them;
- collect diagnostics per worker, merge once, and sort deterministically;
- avoid locks and global mutable compiler state in the per-file hot path; and
- benchmark cold single-file latency as well as warm repository throughput.

Caching should come only after uncached behavior is correct and measured. The
first cache can record that a file was lint-clean for a rule/configuration key or
already formatted for a style key. A key includes the Mojito version, cache
schema, effective configuration, preview state, rule selection, target language
version, and file identity. Metadata provides a fast lookup, with content hashes
used when metadata is ambiguous. Standard input is never cached. Writes use a
temporary file and atomic rename, and a modified file is cached only after its
new metadata is observed.

## Correctness and measurement

Formatter acceptance requires all of the following:

- formatting twice produces exactly the same bytes;
- formatted output reparses successfully;
- its AST is structurally equivalent after ignoring spans and trivia;
- every comment is preserved exactly once unless a documented normalization
  applies;
- invalid input and internal errors never partially rewrite a file; and
- fixtures cover empty constructs, nesting, comments, directives, literals,
  line endings, and boundary line widths.

The formatter should have golden tests and property/fuzz tests. A pinned set of
differential fixtures should compare Mojito with `mojo format`, recording
intentional differences rather than making the installed Mojo binary a test
dependency.

Every lint rule needs positive, negative, suppression, and fix snapshots. Fixes
must reparse, converge, and compose without overlaps. Cross-tool tests should
verify that default lint fixes do not fight the formatter. Cache tests must alter
source, settings, rule selection, and tool versions independently. Parallel and
single-threaded runs must produce byte-identical output.

Benchmarks should report discovery, read, lex, parse, analysis, formatting or
rule execution, printing, sorting, and cache time separately. The baseline set
needs a tiny file for editor latency, a large generated file, and the Mojito
tree for throughput. This makes “do not repeat work” an enforceable property,
not just an aspiration.

## Delivery order

The work should land in usable vertical slices:

1. Specify behavior with Mojo probes and measurements; add the lossless source
   snapshot without changing compiler output.
2. Implement and test the document IR and printer, then format expressions,
   statements, declarations, and modules with idempotence checks.
3. Add comment attachment, suppression directives, atomic file writing, and the
   initial `format` CLI.
4. Add the rule registry, analysis gating, diagnostics, one shared AST/scope
   traversal, and a small set of default lint rules.
5. Add safe fixes, suppressions, configuration, discovery, and stable machine
   output.
6. Measure before adding parallel traversal and caching; add each independently
   with deterministic and invalidation tests.
7. Harden with range formatting, editor integration, fuzzing, and preview style
   and rule evolution.

The detailed, agent-executable version of this sequence is in
[`ruff.md`](../ruff.md).

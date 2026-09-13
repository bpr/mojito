# Mojo asset fixtures

Drop `.mojo` files here to get them exercised by the pipeline (lex → parse → check
→ eval). `tests/corpus_test.rs` turns each file into its own test
(`assets_<folder>::<name>`, plus `vm_ok`/`verify` runs for executable fixtures)
asserting it lands at the outcome the folder names — so **adding coverage is
just putting a file in the right folder**; enumeration is dynamic, no code
changes.

## Folders (by where the pipeline first stops)

| folder            | meaning                                                        |
| ----------------- | ------------------------------------------------------------- |
| `ok/`             | lex + parse + check + eval all succeed                        |
| `parse_error/`    | rejected by the lexer or parser (a syntax gap/error)          |
| `type_error/`     | parses, but the checker rejects it                            |
| `runtime_error/`  | compiles, but fails during VM execution, including explicit late `Unsupported` boundaries |
| `ownership_ok/` / `ownership_error/` | accepted or rejected by the ownership analysis |
| `origin_ok/` / `origin_error/` | accepted or rejected by the origin/escape analysis |

## `extensions/`: Mojito-only language extensions

Every fixture in the `_ok` folders above must compile with the pinned Mojo —
`scripts/sweep-assets-mojo` runs it over them and compares the result against
`conformance/assets-mojo-rejects.tsv`, the burn-down list of the ones it still
rejects. The five error folders are oracled the other way round by the same
script's `--errors` mode; see below. A program that uses a Mojito extension — today, direct `ref` struct
fields (`var f: ref[o] T`), which upstream rejects and may adopt later, and
`Origin._subtree` casts, which upstream parses but rejects at the use; in
future, experiments such as pattern matching or enums — lives under
`assets/extensions/<folder>/` instead, where `<folder>` is the same outcome
folder it would otherwise use (`extensions/ok`, `extensions/type_error`,
`extensions/ownership_error`, …). The same script with `--extensions` asserts
the inverse: the pinned Mojo must reject every extension fixture, so one that
starts compiling is the signal that upstream adopted the extension.

The harnesses run these through the same
groups (named `assets_extensions_<folder>::…`, `vm_ok::extensions::…`, and
so on) and the native parity manifest covers `extensions/ok` and
`extensions/ownership_ok`. When a `ref`-field fixture has a Mojo-valid twin
that spells the storage through `Pointer[T, origin]`, the twin keeps the
same file name in the ordinary folder, with a `ref_field_` prefix respelled
`pointer_field_`.

Grab a Mojo file off the net, decide where mojito should currently land on it,
and drop it in that folder. When mojito gains a feature, a file "graduates" to an
earlier-passing folder (e.g. `parse_error/ → ok/`) — a nice, greppable diff.

## The error folders' oracle

An error fixture is rejected by both compilers, so an exit code proves nothing
about agreement. `scripts/sweep-assets-mojo --errors` therefore compares the
**verdict**: the pinned Mojo must refuse to compile what `parse_error`,
`type_error`, `ownership_error` and `origin_error` claim it refuses, and must
compile-and-trap what `runtime_error` claims. Every fixture has a row in
`conformance/assets-mojo-errors.tsv` recording that verdict, the pin's own first
complaint, and a family saying whether the two compilers really met on the same
defect.

Mojito's message is never compared with the pin's. Outside the parser a Mojito
diagnostic carries no source location, and the two wordings differ for most
fixtures by design; `# expect:` below stays the only pin on Mojito's side.

Two families are worth knowing when adding a fixture:

- A burn-down name (`missing-import`, `self-qualification`, …) means the pin
  rejected the fixture for a spelling it never set out to test, so the defect it
  pins went unexamined. Prefer a spelling the pin accepts.
- `subset` and `divergence` mean the pin *accepts* the program. `subset` is
  deliberate strictness (`docs/non-goals.md`); `divergence` is open work
  (`docs/roadmap.md`).

## Optional: pin the exact error

A file may pin the reported message with a top comment (valid Mojo — the lexer skips
it):

```mojo
# expect: operator '+'
var x: Int = 1 + True
```

The harness then also asserts the error contains that substring.

An `ok` fixture may carry `# requires: discovery` (on its own line) when its
semantics need the `Compiler`'s whole-program discovery/specialization
handoff — e.g. the checker-inferred scalar-range constructor rewrite. The
phase-composed `verify::*` corpus group is non-authoritative for that
handoff (see AGENTS.md), so it skips such fixtures; the authoritative
`vm_ok`/`assets_ok` Compiler trials still compile, verify, and execute them.

An `ownership_ok`/`ownership_error` fixture may carry `# requires: stdlib`
when it names a bundled standard-library type (`StringSpan`, …). Those
corpus groups enter the ownership seam at raw `parse`, which resolves no
module; the directive enters at `link` instead, leaving every later stage of
the seam unchanged.

## Note

Production Mojito, like Mojo, rejects executable statements at file scope and
calls a zero-argument `main()` as the program entry point. No fixture here is a
module-scope snippet any more — the 2026-09-12 error-folder sweep gave the last
four a `main`, because the pinned Mojo rejects file-scope statements outright and
that rejection masked the defect each one pinned. The suite's non-conforming
snippet mode (`Compiler::with_snippet_module_scope`) survives for inline
snippets in `tests/`, not for anything in this tree.

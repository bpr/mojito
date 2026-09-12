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

## `extensions/`: Mojito-only language extensions

Every fixture in the folders above must compile with the pinned Mojo —
`scripts/sweep-assets-mojo` runs it over them and compares the result against
`conformance/assets-mojo-rejects.tsv`, the burn-down list of the ones it still
rejects. A program that uses a Mojito extension — today, direct `ref` struct
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
calls a zero-argument `main()` as the program entry point. Some historical
fixtures remain module-scope snippets and run only through the test suite's
explicit non-conforming snippet mode; new fixtures should be valid Mojo programs.

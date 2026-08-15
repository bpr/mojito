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

## Note

Production Mojito, like Mojo, rejects executable statements at file scope and
calls a zero-argument `main()` as the program entry point. Some historical
fixtures remain module-scope snippets and run only through the test suite's
explicit non-conforming snippet mode; new fixtures should be valid Mojo programs.

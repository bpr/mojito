# Using Mojito

Building and testing the compiler, running programs through the CLI, adding
fixture coverage, running differential conformance against Mojo, and embedding
the compiler as a Rust library. For what the language supports, see the
[feature matrix](features.md); for the project narrative, see the
[overview](overview.md).

## Build And Test

```sh
cargo build
cargo test
cargo clippy --all-targets
```

If your local environment sets `RUSTC_WRAPPER` to a tool that cannot write its
cache, this form is useful:

```sh
env RUSTC_WRAPPER= cargo test
env RUSTC_WRAPPER= cargo clippy --all-targets
```

## CLI Usage

Run a compiler stage over a file:

```sh
cargo run -- <command> [FILE]
```

Commands:

| Command | What it does |
| ------- | ------------ |
| `lex` | print the token stream, one token per line |
| `parse` | print the parsed AST |
| `check` | parse and type-check |
| `own` | parse, type-check, and run ownership analysis |
| `run` | compile and execute on the register VM |
| `emit-mir` | compile and print canonical executable MIR |
| `compile` | native-compile the scalar subset via `--backend pliron` (experimental; requires the `backend-pliron` feature and LLVM 22) |
| `exec` | execute a verified textual MIR artifact |

`FILE` is optional. Use a path, `-`, or omit it to read from standard input:

```sh
cargo run -- parse conformance/fixtures/integer_arithmetic.mojo
cargo run -- check -
echo 'def main(): print(1)' | cargo run -- lex
cargo run -- run assets/ok/list_and_struct.mojo
cargo run -- emit-mir assets/ok/defines_main.mojo | cargo run -- exec -
```

`compile` selects its output with `--emit plir|ll|bc|obj|exe` (default `ll`;
text kinds print to stdout unless `-o PATH` is given, binary kinds require
`-o PATH`) and rejects any program whose reachable-from-`main` call graph
leaves the scalar subset — including `print`, until the native runtime stage:

```sh
cargo build --features backend-pliron   # needs llvm-22-dev (see docs/notes/pliron-stage1.md)
target/debug/mojito compile pure.mojo --backend pliron --emit exe -o pure
```

`check`, `own`, `run`, `emit-mir`, and `compile` link imports. Add repeatable module roots with
`--module-path PATH` (or `-I PATH`) and explicit standard-library roots with
`--stdlib PATH`; either spelling also accepts `--option=PATH`. Resolution checks
the importing file's directory first, then CLI roots in their command-line order,
then mojito's bundled `stdlib/` fallback:

```sh
cargo run -- run -I vendor --module-path ../shared --stdlib ~/mojo/stdlib main.mojo
```

Stage errors are written to standard error with a non-zero exit code, so the CLI
is usable in scripts.

## Writing Programs

Like Mojo, Mojito permits declarations, imports, and compile-time constants at
file scope but rejects executable statements there. Put runtime work in a
function; a zero-argument `main()` is called as the program entry point.

Example:

```mojo
@fieldwise_init
struct Counter:
    var n: Int

    def bump(mut self, by: Int):
        self.n += by

def main():
    var c: Counter = Counter(10)
    c.bump(5)
    print(c.n)
```

Run it:

```sh
cargo run -- run path/to/file.mojo
```

## Fixture Workflow

The easiest way to add coverage is to place `.mojo` files under `assets/`.

The test harness walks these folders:

| Folder | Meaning |
| ------ | ------- |
| `assets/ok/` | program should lex, parse, check, pass ownership analysis, and run |
| `assets/parse_error/` | lexer or parser should reject it |
| `assets/type_error/` | parser accepts it, checker rejects it |
| `assets/runtime_error/` | checker accepts it, VM reports a runtime error |
| `assets/ownership_ok/` | ownership analysis should accept it |
| `assets/ownership_error/` | ownership analysis should reject it |

So adding an accepted language example is usually just:

```sh
$EDITOR assets/ok/my_feature.mojo
cargo test
cargo run -- run assets/ok/my_feature.mojo
```

Negative fixtures can pin part of the expected error with a top comment:

```mojo
# expect: use after move
@fieldwise_init
struct Box:
    var n: Int

def main():
    var a: Box = Box(1)
    var b: Box = a^
    print(a.n)
```

See [`assets/README.md`](../assets/README.md) for the fixture rules.

## Differential Conformance

`conformance/parity.tsv` is the authoritative Mojo comparison ledger. It pins the
reference build and records, for every inventoried manual feature family, the
status, scope, relationship to Mojo, each implementation's behavior, and
evidence. Relations distinguish matching semantics, strict-subset rejection,
true divergence, representation differences, explicit exclusions, and stretch
goals. `conformance/manual-sections.tsv` fixes the official manual inventory
boundary.

The current language target is recorded in
[`docs/mojo-nightly.md`](mojo-nightly.md). Mojito presently tracks
**Mojo 1.0.0b3.dev2026072505 (2026-07-25)**; the audit records nightly language
drift separately from claims of implemented parity.

Shared cases in `conformance/cases.tsv` run under both implementations. They can
assert matching output, matching rejection, a documented strict-subset gap, or a
documented acceptance or output divergence. `scripts/check-parity-manifest`
validates the schema, IDs, classifications, evidence links, fixtures, manual
coverage, the rule that every divergence has an executable differential case,
and the rule that every implemented first-pass match cites differential evidence.

Point the runner at a Pixi project containing the pinned Mojo compiler:

```sh
scripts/conformance --mojo-pixi-manifest /path/to/pixi.toml
```

The same path can be supplied through `MOJO_PIXI_MANIFEST`. The ordinary
`scripts/check` gate does not require Mojo or network access; differential
conformance is an explicit additional gate.

## Library API

The frontend stages are also available as library functions:

```rust
let tokens = mojito::lex(source)?;
let ast = mojito::parse(source)?;
mojito::check(&ast)?;
mojito::check_ownership(&ast)?;
```

For whole-program execution and artifact emission, use the compiler driver:

```rust
use mojito::Compiler;

let compiler = Compiler::default();
let program = compiler.compile_unlinked(source)?;
let artifact = program.emit_mir()?;
let execution = compiler.execute(&program)?;
println!("{}", execution.output);
```

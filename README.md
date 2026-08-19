# mojito

mojito is a small Rust implementation of an evolving subset of
[Mojo](https://mojolang.org/). It is not Mojo, and it is not trying to
compete with Mojo's production compiler: it is a compact compiler playground for
studying the shape of a modern systems programming language compiler. Mojo was
chosen as a target because it is a rich language, with value semantics,
ownership/borrowing, ASAP destruction, generics, overloading, and compile-time
execution — interesting features associated with C++, Rust, and Zig. Those
semantics are enforced as explicit compiler analyses over a small MIR, with a
register VM as the executable semantic oracle, and the goal is honest subset
semantics: features are usually parsed before they are fully supported, and
unsupported semantics fail cleanly instead of producing a wrong answer.

## Project Goals

- Parse all of current Mojo and report syntax errors
- Approach semantic parity with a single-threaded, CPU-only subset of current
  Mojo. All mojito programs should be runnable by Mojo, though platform-specific
  Mojo programs will remain outside the target
- Keep the register VM as the executable semantic oracle while adding a stable
  textual MIR/VM assembly format and, later, native backends — Pliron and
  Cranelift first, with a possible C or C++ backend

## Status

The [feature matrix](docs/features.md) is the authoritative record of what is
executable, checked-only, parse-only, or unsupported. The
[project overview](docs/overview.md) narrates the current status, the
deliberate gaps relative to Mojo, and the development direction. Mojito
currently tracks the pinned Mojo nightly recorded in
[`docs/mojo-nightly.md`](docs/mojo-nightly.md).

## Pipeline

```text
source
  -> lex
  -> parse
  -> module link
  -> comptime elaboration
  -> check
  -> HIR CFG
  -> MIR
  -> ownership / borrow / liveness analysis
  -> drop elaboration
  -> register VM
```

The `Compiler` driver owns this ordering for whole-program use; individual
stage APIs remain available for syntax tools and compiler development. See
[the architecture guide](docs/architecture.md) for phase design and
[the symbol map](docs/symbol-map.md) for code navigation.

## Build

```sh
cargo build
cargo test
cargo clippy --all-targets
```

If your environment sets `RUSTC_WRAPPER` to a tool that cannot write its cache,
prefix commands with `env RUSTC_WRAPPER=`.

## Quick Start

Run a compiler stage over a file, `-`, or standard input:

```sh
cargo run -- <command> [FILE]
```

| Command | What it does |
| ------- | ------------ |
| `lex` | print the token stream, one token per line |
| `parse` | print the parsed AST |
| `check` | parse and type-check |
| `own` | parse, type-check, and run ownership analysis |
| `run` | compile and execute on the register VM |
| `emit-mir` | compile and print canonical executable MIR |
| `exec` | execute a verified textual MIR artifact |

For example:

```sh
echo 'def main(): print("Hello from mojito")' | cargo run -- run
cargo run -- run assets/ok/list_and_struct.mojo
cargo run -- emit-mir assets/ok/defines_main.mojo | cargo run -- exec -
```

See [usage](docs/usage.md) for module-root options, how to write and run
programs, the fixture workflow, differential conformance against Mojo, and the
Rust library API.

## Documentation

- [`docs/usage.md`](docs/usage.md) — building, CLI usage, writing programs,
  fixtures, conformance runs, and the library API
- [`docs/overview.md`](docs/overview.md) — status snapshot, gaps relative to
  Mojo, pipeline tour, and development direction
- [`docs/features.md`](docs/features.md) — authoritative feature support matrix
- [`docs/architecture.md`](docs/architecture.md) — pipeline invariants and
  phase design
- [`docs/symbol-map.md`](docs/symbol-map.md) — symbol-level ownership and
  navigation map
- [`docs/grammar.md`](docs/grammar.md) — accepted surface syntax
- [`docs/vm-instruction-set.md`](docs/vm-instruction-set.md) — VM transition
  and instruction model
- [`docs/mir-text-format.md`](docs/mir-text-format.md) — textual MIR artifact
  format
- [`docs/roadmap.md`](docs/roadmap.md) — current direction and pending work
- [`assets/README.md`](assets/README.md) — test fixture rules
- [`CHANGELOG.md`](CHANGELOG.md) — user-visible history

## Links

- [`Mojo Language Reference`](https://mojolang.org/docs/reference/)
- [`Mojo Manual`](https://mojolang.org/docs/manual/)
- [`Mojo By Example`](https://ruhati.net/mojo/index.html) - an excellent quick tutorial
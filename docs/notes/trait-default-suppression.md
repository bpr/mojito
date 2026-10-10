# When a conformer's method suppresses a trait default

Book destination: chapter 15 (Traits, Bounds, and Associated Facts), as a
worked case of a subset position Mojito keeps until its symbol scheme can
carry the difference.

## What upstream does

A struct that conforms to a trait inherits each default it does not
provide. Upstream decides "does not provide" by *satisfaction*: while the
struct's conformances resolve (`signatureResolveDefaultTraitFnStubs`,
`Traits.cpp`), each defaulted trait function is checked against the
struct's own overload set (`filterOverloadSetForValueType`). Only a member
that satisfies the default's signature suppresses it. Otherwise
`StructEmitter::synthesizeDefaultTraitMethodWrapper` builds the wrapper
beside whatever the struct spelled, from the trait function's resolved
signature.

So, at the pin (`mojo-2026092105`):

```mojo
trait T:
    def f(self) -> Int:
        return 1

struct S(T, Movable):
    var x: Int
    def __init__(out self):
        self.x = 0
    def f(self) -> String:
        return "s"

def main():
    _ = S()
```

is accepted: `S` has two `f`s, its own returning `String` and the default's
wrapper returning `Int`. Calling `S().f()` is then rejected as an ambiguous
call, since both take no argument.

## What Mojito does

Mojito suppresses a default by its *parameter shape*: a spelled method
whose name and parameter overload key equal the default's
(`trait_defaults::inherited_defaults`, keyed by `overload_key`) takes its
place, whatever it returns. `S.f` above therefore suppresses the default,
and the conformance check then reports that `S.f` "does not match the
signature required by trait 'T'". Mojito rejects a program the pin
accepts; it never accepts one the pin rejects, because a call of either
`f` is rejected at the pin anyway.

## Why the gap stays for now

Following upstream needs two members of one parameter shape on one
struct. Mojito's callable identity cannot tell them apart:

- `SignatureKey` (`mojito-symbol`) spells a method's parameter types, not
  its result, so both `f`s lower to the same `S.f$ov$...` symbol.
- The `OverloadSets` prescan decides whether a name is overloaded by
  counting declarations per arity, and qualifies every declaration of an
  overloaded arity the same way.

Closing the gap is a symbol-scheme change (result-aware identity, or a
wrapper symbol upstream-style distinct from the spelled member), filed as
R570 on `docs/roadmap.md`'s `calls` track.

# `enum` in Mojito

The pinned Mojo (`1.2.0.dev2026101705`, source `cfd79ca736`) implements
enums under the experimental keyword `__enum`: its lexer registers
`kw___enum`, `StructEmitter::synthesizeEnumMembers` declares an enum's
members, `std/builtin/_enum.mojo` implements them, and the pinned binary
runs `Mojo/test/mojo-integration/enum.mojo`, `enum_storage.mojo`, and
`enum_many_cases.mojo`. Mojito spells the declaration `enum`, as upstream's
concept proposal (`Mojo/proposals/enums.md`) does, and does not accept
`__enum` (owner's decision, 2026-10-25). Everything else follows the pin, so
`enum` is a kept extension (`docs/non-goals.md`): its fixtures live under
`assets/extensions/`, where the pin rejects them for the spelling alone, and
each was checked against the pin's output under `__enum`. The proposal's
several-subpattern payload match (`.rgb(r, g, b)`) is not at the pin, and
Mojito follows the pin (`.rgb((r, g, b))`).

`enum` is a contextual keyword: the lexer emits it only where a statement
starts with `enum` followed by a name that is not a keyword (`enum Color:`),
so a variable or parameter named `enum` keeps working, as at the pin.

## What the pin generates, and what Mojito generates

For the pin's

```mojo
__enum Token:
    case eof
    case ident(String)
    case span(Int, Int)
```

upstream declares the struct `Token(__Enum)` with `_enum_case_names`,
`_enum_case_types`, one field `__enum_storage: __EnumStorage[case_types]`,
`comptime eof: Token = Token.__enum_init[0, NoneType]()`, and static
constructors calling `Token.__enum_init[i, Case](payload0^, ...)`;
`__Enum`'s defaults supply `__enum_init` (which writes each payload element
in place), `__get_enum_storage`, `_get_enum_discriminant`, and
`_unsafe_get_enum_payload`.

Mojito generates the same names and lists, written as Mojo and parsed
(`mojito_parser::enums`), with these differences:

- **The four `__Enum` members are generated on each enum**, not inherited
  as trait defaults: upstream's defaults are a `@staticmethod` with an `out`
  result over a payload pack and `ref`-returning methods, which Mojito's
  trait defaults cannot hold yet (R942, R549, R571), and `__enum_init`'s
  element-wise write needs a runtime `consume_elements` handler (R1124) and a
  capture-list closure as a value (R1378). So each case constructor writes
  its own storage in place, and `__enum_init` takes a built storage; its
  only caller is a case without a payload. The generated members' bodies
  carry `std.builtin._enum`'s source, the module upstream writes them in, so
  they may mark the result initialized and return references into the
  storage.
- **`__EnumStorage[*Ts]` is a compiler intrinsic**, the same
  `TyKind::Variant` the bundled `Variant`'s `__VariantStorage` is, selected
  by case index (`__EnumStorage[...](payload, index=i)`, `storage[i]`,
  `get_discriminant()`, MIR `variant.tag`) and never by type, so cases may
  share a payload type. Upstream's is a library struct over `!kgen.variant`
  with `pop.variant.discr_gep` and `pop.variant.bitcast`, which Mojito's
  `__mlir_op` does not accept. Its tag is the variant's `u32`, where
  upstream's `__enum_discriminant_dtype` picks `u8`, `u16`, or `u32` by case
  count; nothing but `size_of` observes it.
- **The default `Writable` text** is generated per enum (`write_to`,
  `write_repr_to`), with upstream's `_enum_reflection_write_to` shape, since
  Mojito's reflective default is a compiler builtin (R424).

A case without a payload is a `comptime` member whose value the check
freezes as the enum over a frozen variant (`CtValue::Variant`), which both
backends materialize; as at the pin, using one at run time needs an
`ImplicitlyCopyable` enum.

## Evidence

The pin's own integration test, adapted where Mojito lacks an unrelated
feature (bindings in `|` alternatives, R1246; `String`'s variadic
constructor; `std.iter.map`), splits into the `assets/extensions/ok/enum_*.mojo`
fixtures, whose output matches the pin's on the VM and natively except where
noted in `docs/features.md`. The diagnostic fixtures under
`assets/extensions/parse_error/enum_*.mojo` and `assets/extensions/type_error/enum_*.mojo` carry
the pin's words.

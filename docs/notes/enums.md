# `enum` and `union` in Mojito

The pinned Mojo (`1.2.0.dev2026101705`, source `cfd79ca736`) implements
one sum-type declaration under the experimental keyword `__enum`: its lexer
registers `kw___enum`, `StructEmitter::synthesizeEnumMembers` declares an
enum's members, `std/builtin/_enum.mojo` implements them, and the pinned
binary runs `Mojo/test/mojo-integration/enum.mojo`, `enum_storage.mojo`, and
`enum_many_cases.mojo`. Upstream's enum proposal (`Mojo/proposals/enums.md`,
2026-11-07) splits that declaration in two:

- An `enum` declares payload-free cases (`case red, green, blue`). Each case
  is a value, never called.
- A `union` declares cases that each carry one payload in parentheses:
  `case eof()` carries `Tuple[]`, `case ident(String)` a `String`, and
  `case span(Int, Int)` a `Tuple[Int, Int]`.

Mojito implements the split and spells the declarations `enum` and `union`,
without the leading underscores of upstream's newer sources (`__union`
appears in upstream's formatter on 2026-11-02), and does not accept
`__enum` or `__union` (owner's decisions, 2026-10-25 and 2026-11-15). Both
are kept extensions (`docs/non-goals.md`): their fixtures live under
`assets/extensions/`, where the pin rejects them.

Both keywords are contextual: the lexer emits one only where a statement
starts with `enum` or `union` followed by a name that is not a keyword
(`enum Color:`), so a variable or parameter named `enum` or `union` keeps
working, as at the pin.

## What the proposal says, and what Mojito does

- **Enum cases are values.** `Color.red` and `.red` read the case;
  `Color.red()` is an error that names the fix.
- **Enums are implicitly copyable.** Mojito adds `ImplicitlyCopyable` (which
  brings `Copyable` and `Movable`) to every enum's conformances. The
  proposal's opt-out spelling (`not Movable`) does not parse yet (R2238).
- **Enums derive what they list.** An enum listing `Equatable` or
  `Comparable` gets an `__eq__` comparing its cases' discriminants,
  `Comparable` adds `__lt__` over declaration order, `Hashable` gets a
  `__hash__` hashing the discriminant, and `Writable` writes `Name.case`.
  A member the enum writes itself wins.
- **Union cases are constructors.** A union case is a static method taking
  the payload's elements; unapplied (`Poll[Int].pending`) it is a function
  value.
- **Union patterns always take parentheses.** Over a `union`, `.eof()`
  matches the empty payload, `.ident(name)` binds the payload, and
  `.span(a, b)` is a tuple pattern over a `Tuple[Int, Int]` payload, so
  `.span((a, b))` and `.span(pair)` match too. A bare `.eof` and a keyword
  subpattern are errors; a struct payload destructures by its fields
  (`.at({start=x, end=y})`).
- **No stored state.** A `var` member is an error in either declaration.
- **Traits.** The proposal bases matching and reflection on three traits,
  which Mojito bundles in `stdlib/std/builtin/enum_like.mojo` in place of
  the pin's one `EnumLike` protocol (`_enum_case_names`, `_enum_case_types`,
  `_get_enum_discriminant`, `_unsafe_get_enum_payload`):
  - `SumType` declares `_sum_case_names` and `_get_sum_discriminant`.
  - `EnumLike(SumType)` adds nothing. Every `enum` conforms, and so do
    `Bool` and `FastMathFlag`.
  - `UnionLike(SumType)` adds `_union_case_types`,
    `_union_elt_type_for_case`, and `_unsafe_get_union_payload`. Every
    `union` conforms, and so do `Optional` and `OptionalReg`, whose `None`
    case carries `Tuple[]` and is matched `.None()`.
  - A case pattern asks `SumType` for the case and `UnionLike` for the
    payload (`checker/patterns.rs`). A parenthesized pattern over a subject
    that is not `UnionLike` is an error, and so is a bare one over a subject
    that is.
  - `SumType` keeps the pin's defaulted `_enum_is_exhaustive`, which the
    proposal's sketch does not mention: the pin and upstream's newer sources
    both have open enums, and coverage reads the flag.
  - A program written against the pin's protocol, or matching a bare
    `.None`, is rejected. Upstream had not implemented the three traits by
    2026-11-13 (`e6e596ce2c`), so the proposal's text is the only source.

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

Mojito generates the storage and the constructors the same way, written as
Mojo and parsed (`mojito_parser::enums`), under the proposal's member names:
`_sum_case_names` and `_get_sum_discriminant` on both declarations, and
`_union_case_types` and `_unsafe_get_union_payload` on a union alone. An
enum case is the pin's case without a payload: a `comptime` member over a
`NoneType` storage alternative, with no case type listed. A union case is
the pin's case with one: a static constructor, whose `()` payload is
`Tuple[]`. No payload type marks an absent payload, so `NoneType` is a
payload type like any other. `__Enum` refines `EnumLike` and `__Union`
refines `UnionLike`. The remaining differences from the pin:

- **The `__Enum` members are generated on each declaration**, not
  inherited as trait defaults: upstream's defaults are a `@staticmethod`
  with an `out` result over a payload pack and `ref`-returning methods,
  which Mojito's trait defaults cannot hold yet (R942, R549, R571), and
  `__enum_init`'s element-wise write needs a runtime `consume_elements`
  handler (R1124) and a capture-list closure as a value (R1378). So each
  case constructor writes its own storage in place, and `__enum_init` takes
  a built storage; its only caller is an enum case. The generated members'
  bodies carry `std.builtin._enum`'s source, the module upstream writes
  them in, so they may mark the result initialized and return references
  into the storage.
- **`__EnumStorage[*Ts]` is a compiler intrinsic**, the same
  `TyKind::Variant` the bundled `Variant`'s `__VariantStorage` is, selected
  by case index (`__EnumStorage[...](payload, index=i)`, `storage[i]`,
  `get_discriminant()`, MIR `variant.tag`) and never by type, so cases may
  share a payload type. Upstream's is a library struct over `!kgen.variant`
  with `pop.variant.discr_gep` and `pop.variant.bitcast`, which Mojito's
  `__mlir_op` does not accept. Its tag is the variant's `u32`, where
  upstream's `__enum_discriminant_dtype` picks `u8`, `u16`, or `u32` by case
  count; nothing but `size_of` observes it.
- **The default `Writable` text** is generated per declaration (`write_to`,
  `write_repr_to`), with upstream's `_enum_reflection_write_to` shape, where
  upstream's enum inherits `Writable`'s own bodies, which reach
  `_enum_reflection_write_to` (R1409). The bundled
  `_enum_reflection_write_to` writes any `SumType`'s case name and a
  `UnionLike`'s payload after it. A tuple payload is written element
  by element (`Token.span(1, 2)`), as upstream's newer sources write it.

An enum case is a `comptime` member whose value the check freezes as the
enum over a frozen variant (`CtValue::Variant`), which both backends
materialize.

## Evidence

The pin's own integration test, adapted where Mojito lacks an unrelated
feature (bindings in `|` alternatives, R1246; `String`'s variadic
constructor; `std.iter.map`), splits into the
`assets/extensions/ok/{enum,union}_*.mojo` fixtures, whose output matches the
pin's on the VM and natively for the same programs written with `__enum`,
except where noted in `docs/features.md`. The fixtures for what the
proposal adds (`enum_derived_conformances`, `enum_nullary_constants`,
`union_tuple_payload_patterns`, the `union_*`/`enum_case_*` diagnostics, and
the trait fixtures `enum_like_*`, `union_like_*`, and `match_*` under
`assets/extensions/`) have no pin to compare against; their words are
Mojito's where upstream has published none. The `match_*` ones ran at the
pin under its protocol, with `.None` for `.None()`, before they moved.

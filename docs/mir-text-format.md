# Mojito Textual MIR Format, Version 1.75

This document is the normative specification of Mojito's textual verified-MIR
artifact. The Rust data model in `src/mir.rs` and `src/mir/ir.rs` remains the
in-memory authority; this format is its stable inspection and interchange
boundary. `docs/vm-instruction-set.md` explains execution semantics, while this
document defines syntax and serialized data.

Version 1.75 is implemented end to end for inspection and loading: canonical
in-memory disassembly, full-grammar assembly parsing, and artifact-loading
verification (`mir::text::load_artifact`, which reports canonical-verifier
findings at artifact source spans). Lossless print → parse → print round trips
are enforced byte-for-byte over the drop-elaborated MIR of every executable
corpus fixture by the `roundtrip::*` group of `tests/corpus_test.rs`, and
`mojito exec [FILE]` loads a verified artifact, elaborates it to concrete MIR
from `main` and module initialization, and runs it on the register VM
(`artifact::run_artifact`). The artifact itself stays drop-elaborated MIR,
which may be generic; it is always elaborated before it runs.

## Compatibility

Every artifact begins with exactly:

```text
mojito-mir 1.81
```

The writer emits 1.81. The reader accepts 1.0 through 1.81; *Schema 1.0*
below says how a 1.0 artifact is read, and *Binder identity* under *Types*
how a binder without an identity is.

The two unsigned decimal components are major and minor versions. A consumer
must reject an unknown major version. Within major version 1, a newer minor may
add fields only to an `optional { ... }` record or add named capabilities to the
header. Unknown optional fields and capabilities may be skipped; unknown
required fields, types, instructions, terminators, or enum tags are errors. A
semantic or executable addition therefore requires consumer support and cannot
be hidden in optional metadata. Removing or changing existing syntax requires a
new major version.

Minor version 1 respells one value family — compile-time parameter
expressions — and is therefore not an optional addition: a 1.0 consumer
rejects a 1.1 artifact at the header, which is the intended failure, and a 1.1
consumer reads both. Every other spelling, the concrete `ct_*` values
included, is byte-identical between the two.

Minor version 2 gives every binder record — a `param` type and a
`type_param`/`value_param` declaration — the `owner` and `slot` fields that
carry its declaration's identity, the same fields a `param_decl_ref` already
carries. A 1.1 consumer rejects them as unknown required fields, which is the
intended failure; a 1.2 consumer reads 1.1 by giving each binder an identity
per spelling (*Binder identity* under *Types*).

Minor version 3 carries that identity on the two instruction operands that
still named a binder by spelling. `type.construct` gains `owner` and `slot`
beside `param`, and a `param_arg` gains `binder`: `absent`, or the
`present(binder { owner, slot, name })` of the enclosing declaration's type
binder the argument forwards (`hash[Self.H](key)`). A 1.2 consumer rejects both as
unknown fields, which is the intended failure. A 1.3 consumer reads an older
`type.construct` by its destination register's type (*Binder identity* under
*Types*), and an older `param_arg` as forwarding no recorded binder.

Minor version 4 carries that identity on the operands of a `where` clause and
on a callable default's parameter. The `param` field of `conforms`,
`conforms_pack`, `pack_predicate`, and `pack_contains`, and the operand of
`operand_param`, `operand_pack_length`, and `default_parameter`, is a
`binder { owner, slot, name }` record where it was a bare symbol. A 1.3
consumer rejects the record, which is the intended failure. A 1.4 consumer
reads an older bare symbol as the binder of that spelling in the parameter
list declaring the clause, or as an unbound reference when that list declares
none (*Binder identity* under *Types*).

Minor version 5 carries that identity on a deferred slot. The operand of
`ct_deferred` is a `binder { owner, slot, name }` record where it was a bare
symbol, and an elaborator marker, which an older artifact spelled as a
deferred slot, is `ct_marker`. A 1.4 consumer rejects both, which is the
intended failure. A 1.5 consumer reads an older bare symbol as an unbound
reference of that spelling, or as the marker the symbol spells (`$local`,
`$type`, `$tuple-origin:<id>:<imm|mut|param>`).

Minor version 6 carries that identity on a pack query. The `pack` field of
`param_pack_query` is a `binder { owner, slot, name }` record where it was a
bare symbol. A 1.5 consumer rejects the record, which is the intended
failure. A 1.6 consumer reads an older bare symbol as an unbound reference of
that spelling.

Minor version 7 carries the expression a value argument denotes. A
`param_arg` gains `expr`: `absent`, or the parameter expression over the
enclosing declaration's value binders that the argument was built from
(`successor[n, 1 + n]()` records `1 + n`). Its `value` register is unchanged.
A 1.6 consumer rejects the field, which is the intended failure. A 1.7
consumer reads an older `param_arg` as recording no expression.

Minor version 8 carries the spelled receiver of a static call. A `call` gains
`receiver`: `absent`, or the type the call's receiver spells on a generic
struct, in the caller's binder scope (`Pair[Self.U].count()` records
`Pair[U]`). No runtime argument of such a call need carry the struct's
parameters, so the elaborator binds them from this type; a resolved call in
concrete MIR records `absent`. A 1.7 consumer rejects the field, which is the
intended failure. A 1.8 consumer reads an older `call` as recording no
receiver.

Minor version 9 carries a struct member's `where` clauses
(`availability` on a declaration), each struct's conformance rows
(`conformances`), and the declared traits (`traits`). An older artifact reads
as carrying none.

Minor version 42 adds each struct's declared conformances (`declared`, the
same `conformance` records, in declaration order) and records each declared
trait as `trait { name: symbol, refines: [symbol...] }`, its parents in
declaration order, where `traits` held bare names; a struct's
`conformances` hold only its lifecycle rows. An older artifact's struct
reads as declaring nothing, and its trait as refining nothing.

Minor version 44 adds each trait's own requirements, defaulted or not, in
declaration order, one per overload:
`trait { name: symbol, refines: [symbol...], requirements: [requirement {
name: symbol, arity: int }...] }`, `arity` counting the spelled parameters
the receiver aside. A lookup of `Trait.method` reads a trait's overload set
from them. An older artifact's trait reads as requiring nothing.

Minor version 45 drops a `param_arg`'s `binder`: a type argument is
compile-time data on the call, carried by its instantiated arguments, and
its slot holds neither a register nor the binder it forwards. A 1.45
consumer reads an older `param_arg`'s `binder` and ignores it.

Minor version 46 carries a struct's parameterized associated types
(`parameterized_associated_types` on a `struct` record, after
`associated_types`): a list of `parameterized_associated_type { name:
symbol, params: [param...], type: type }`, sorted by name. `params` lists
the member's explicit parameters in application order, each
`type_param(binder)`, `value_param(symbol)`, or `origin_param(int)` keyed as
`type` names it. The elaborator answers `C.IteratorType[origin_of(xs)]` from
them once `C` is bound to an instance, and elaborated MIR carries an empty
list. An older artifact reads as carrying none.

Minor version 47 adds a loan's `setup` flag, after `shared`. A setup loan
holds a by-value call operand's reference-producing lvalue (an accessor or
a `ref`-returning call) until its call's setup reads it: it keeps its owner
live and carries its interior generation, excludes no access and no other
loan, and every read through its reference reads its owner. An older
artifact's loans read as not setup loans.

Minor version 48 gives each trait requirement its signature, after
`arity`: `dispatch`, the `__trait_dispatch.*` symbol a call through a bound
selects it by, then `param_names`, `param_types` (`Self` spelled `Self`),
`param_conventions`, and `variadic_index`, as a `decl` record spells them.
Ownership analysis passes a call through a bound its arguments by these
conventions, as it does a concrete call by its callee's declaration. An
older artifact's requirement reads with an empty `dispatch` and no
parameters, so such a call passes its arguments by address.

Minor version 10 adds the signature origin `sig_carried(type)`: the source of
a transfer effect whose body stores a value of a type over its binders. It
stands for every origin that type names once the binders are bound. A 1.9
consumer rejects the spelling, which is the intended failure.

Minor version 11 adds the instruction `type.name`: the unqualified spelling
of a type that names a compile-time parameter, which only a template holds.
The elaborator replaces it with a string `const` spelled from the
substituted type, so elaborated MIR holds none. A 1.10 consumer rejects the
instruction, which is the intended failure.

Minor version 12 carries a struct's unparameterized associated types
(`associated_types` on a `struct` record): a list of
`associated_type { name, type }`, sorted by name, each type over the
struct's own binders. The elaborator solves `C.Element` from them once `C`
is bound to an instance, and elaborated MIR carries an empty list. An
older artifact reads as carrying none.

Minor version 13 carries the compile-time arguments the checker solved for a
call of a generic `def`: a `call` gains `instantiated_args`, a list of type
arguments in declaration order and in the caller's binder scope
(`bytes[Int]()` records `[Int]`). A type parameter no runtime parameter or
result spells is bound from here by the elaborator; a resolved call in
concrete MIR records an empty list. An older artifact reads as carrying none.

Minor version 15 adds the `comptime_branch` terminator: a `comptime if` a
generic body keeps, whose condition is a constraint over the body's binders
rather than a register. A 1.14 consumer rejects it at the terminator, which
is the intended failure; every other spelling is unchanged, and the reader
still accepts every earlier minor.

Minor version 17 carries a whole pack spread into a callee's collector: a
`call` gains `spread`, `absent` or `present(N)`, the position in `args` of
the argument that is the caller's collector (`show(*args)`), a
`VariadicPack` over a pack still a parameter, which only a template holds.
The elaborator replaces the argument with the bound pack's element places,
so elaborated MIR records `absent`. An older artifact reads as recording no
spread.

Minor version 18 lets a SIMD instruction's lane slots stay symbolic: the
`dtype` and `width` fields of `simd.make`, `simd.cast`, and `simd.bits` take
`ct_expr(...)`, the parameter expression a generator names (`Scalar[dt](x)`
in a `DType`-keyed `def`), exactly as the `simd` type record's slots do. The
elaborator closes them per instance, so elaborated MIR holds known slots. A
1.17 consumer rejects the spelling, which is the intended failure.

Minor version 19 carries the arguments of a type parameter's construction
through its bound's initializer: `type.construct` gains `kwargs` and
`kwarg_places`, spelled and aligned as a `call`'s, which `T(copy=x)` fills
with the `Copyable` initializer's borrowed source and `T()` leaves empty.
The elaborator replaces the instruction with the bound type's construction,
so elaborated MIR holds none. An older artifact reads as constructing with
no argument.

Minor version 20 adds the constant `param(param-expr)`: a compile-time query
read as a runtime value (`Ts.length`, `len(Ts)`, `Ts.contains[X]()`,
`Ts.all_conforms_to[T]()` of a `def`'s own pack), upstream's
`kgen.param.constant` with a symbolic attribute. The elaborator folds it to
an `int` or `bool` per instance, so elaborated MIR holds none. A 1.19
consumer rejects the spelling, which is the intended failure.

Minor version 21 lets `type.construct` build an element of a type pack
(`Ts[i]()`, `Self.Ts[i]()`): it gains `element`, `absent` for a type
parameter's own construction, or the `present(param_arg { … })` whose
`value` register a reader may inspect and whose `expr` is the index the
elaborator evaluates, a literal one included. `param` then names the pack.
The elaborator replaces it with the selected element's default
construction, so elaborated MIR holds none. A 1.20 consumer rejects the
field as unknown, which is the intended failure; an older artifact reads as
constructing the parameter itself.

Minor version 22 carries a method's own solved compile-time arguments: a
`call.method` gains `instantiated_args`, as a `call` did in version 13, in
declaration order and in the caller's binder scope (`s.name[Int]()` records
`[Int]`). The elaborator binds the method's own binders from them, and a
resolved call in concrete MIR records an empty list. An older artifact reads
as carrying none.

Minor version 23 adds the instruction `value.rebind { dest, value }`:
`rebind[Dest](x)` read as a value in a generator, `dest` typed `Dest` and
`value` typed the operand's own type, as upstream's `kgen.rebind`. The
elaborator asserts the two types equal per instance, after deciding its
`comptime_branch`es, and erases the instruction, so elaborated MIR holds
none and a mismatch fails the instance. A 1.22 consumer rejects the
instruction as unknown. A rebound place carries its own step from minor
version 37.

Minor version 24 carries a whole pack spread into a method's collector: a
`call.method` gains `spread`, as a `call` did in version 17, the position in
`args` of the caller's collector (`Sink().take(*args)`). The elaborator
replaces it with the bound pack's element places, so elaborated MIR records
`absent`. An older artifact reads as recording no spread.

Minor version 27 adds the constant `value(ct-value)`, a closed vector- or
struct-typed parameter value read as a runtime value (`Self.key` of an
`AHasher[key: U256]` instance, `Self.e` of a `Tagged[e: Extent]` one): the
elaborator folds a binder read to it, the VM materializes it, and native
lowering builds the vector or the fieldwise struct. It adds the parameter
expression `param_field { base, name, type }`, a field of a struct-typed
parameter value that is still a parameter (`Self.e.rows`), which folds once
its base is constant. A 1.26 consumer rejects both spellings, which is the
intended failure.

Minor version 29 adds the parameter expressions `param_list_tabulate {
count, element }` and `param_list_concat { lists }`, the lists a
`TypeList.reverse()` and a `TypeList._concat[...]()` over packs still open
compute, and admits one as the sole argument of a struct type that spreads
it (`Tuple[*TypeList._concat[Self.Ts.values, OtherTs.values]()]`, the result
type of `Tuple.concat`). The elaborator closes it to the instance's element
types. A 1.28 consumer rejects both spellings, which is the intended failure.

Minor version 32 adds `enclosing` to a function declaration: `absent`, or
the `present(symbol)` lowered name of the function or method a nested `def`
or lambda is declared in. The nested body may name that declaration's
binders, and its enclosing declaration's in turn, without capturing them;
the elaborator instantiates it under the enclosing instance's bindings, and
concrete MIR records `absent`. A 1.31 consumer rejects the field, which is
the intended failure. A 1.32 consumer reads an older declaration as nested in
nothing.

Minor version 33 adds the instruction `type.construct_expr { dest, type }`:
the construction of the type a compile-time parameter expression denotes,
through the `Defaultable` initializer the checker proved of it (`FT()` and
`types[i]()` over `reflect[T].field_types()`). Only a template holds one;
the elaborator writes the closed type's default construction, so elaborated
MIR has none. A 1.32 consumer rejects the instruction, which is the
intended failure.

Minor version 35 adds `region_binders` to a function declaration: the
`param_decls`-shaped binders of the compile-time regions of the enclosing
body that a nested function is declared in, today the index of each
enclosing `comptime for`, outermost first. The nested body and signature
may name them; the elaborator instantiates the body once per unrolled copy
of the region, and concrete MIR records `[]`. A 1.34 consumer rejects the
field, which is the intended failure. A 1.35 consumer reads an older
declaration as declared in no region.

Minor version 36 carries a struct's compile-time value members
(`associated_values` on a `struct` record): a list of
`associated_value { name, value }`, sorted by name, each value a parameter
expression over the struct's own binders (`7`, `Self.n + 1`), and adds the
parameter-expression node `param_type_member { subject, name, type }`, the
value member `name` of a subject type that is still a parameter (`T.K`),
typed by the member's declared value type. The elaborator answers the node
from the bound instance's `associated_values`, so elaborated MIR carries
neither. A 1.35 consumer rejects the node, which is the intended failure; a
1.36 consumer reads an older struct as carrying no value members.

Minor version 37 adds the projection `rebind`: `rebind[Dest](place)` read or
written as a place in a generator, the step typed `Dest` and placed after the
operand's own projections, so a later projection addresses `Dest`, as
upstream's reference overload applies `kgen.rebind` to the pointer it
returns. The step selects no storage. The elaborator asserts its type equal
to the one it applies to per instance and erases the step, so elaborated MIR
holds none. A 1.36 consumer rejects the projection, which is the intended
failure.

Minor version 40 carries a whole pack spread into a generic nested `def`'s
collector: a `call.indirect` gains `spread`, as a `call` did in version 17,
the position in `args` of the caller's collector (`count(*args^)`). The
elaborator replaces it with the bound pack's element places before it names
the lifted body's instance, so elaborated MIR records `absent`. An older
artifact reads as recording no spread.

Artifacts are UTF-8, use LF logical newlines, end in exactly one LF, and contain
no byte-order mark. The header is followed by one artifact record:

```text
artifact {
  features: [],
  files: [],
  structs: [],
  decls: [],
  functions: []
}
```

Version 1.0 defines no capabilities, so `features` is canonically `[]`.
`MirProgram::invariant_errors` is deliberately absent: findings are a local
verifier result, never trusted artifact input.

## Lexical Grammar

The notation below is EBNF. Literal punctuation is quoted.

```text
digit       = "0" … "9" ;
hex         = digit | "a" … "f" ;
uint        = "0" | ("1" … "9"), { digit } ;
sint        = [ "-" ], uint ;
bare        = ("A" … "Z" | "a" … "z" | "_"),
              { "A" … "Z" | "a" … "z" | "_" | digit } ;
tag         = bare, { ".", bare } ;
string      = '"', { scalar | escape }, '"' ;
escape      = '\\"' | '\\\\' | '\\n' | '\\r' | '\\t' |
              '\\u{', hex, { hex }, '}' ;
symbol      = bare | string ;
reg         = "%r", uint ;
var         = "$v", uint ;
block       = "bb", uint ;
file-id     = "file", uint ;
list        = "[", [ value, { ",", value }, [ "," ] ], "]" ;
record      = tag, "{", [ field, { ",", field }, [ "," ] ], "}" ;
field       = bare, ":", value ;
option      = "absent" | "present", "(", value, ")" ;
```

Spaces, tabs, and newlines separate tokens. `#` begins a comment through the
next LF outside a string. Comments are accepted but canonical output emits none.
Keywords listed by `mir::text::RESERVED_WORDS` cannot be bare symbols.

Strings contain Unicode scalar values. Canonical output escapes quote,
backslash, LF, CR, and tab with their short forms, every other control scalar as
lowercase `\u{hex}` without leading zeroes, and leaves other scalars literal.
This grammar is independent of Mojo source literals. Symbols use a bare spelling
only when `mir::text::is_bare_identifier` permits it; otherwise they use a
string.

All lists are ordered and length-delimited by brackets. `absent`, `present(x)`,
empty lists, empty strings, `none` values, and zero are distinct.

## Canonical Ordering and Identities

- `%rN`, `$vN`, `bbN`, and `fileN` preserve their numeric identities. A parser
  must not renumber them.
- File records are sorted by `(path, module)` and assigned dense IDs from zero.
- Struct and function declarations are sorted by their `name`/`lowered_name`
  symbol bytes. Struct fields and function parameters remain declaration-ordered.
- Functions retain `MirProgram::functions` order; names must be unique.
- Blocks retain vector order and must be densely named `bb0..bbN`.
- Numeric maps (`var_types`, `reg_types`, locations) are sorted by numeric key.
- Sets and semantic maps are sorted by their serialized key. Origin unions and
  capture sets use their already-canonical semantic order.
- Record fields appear in the order specified here. A version 1.0 canonical
  emitter never omits a required field, even when it is empty or false.
- These ordering rules bind the emitter. A parser requires the dense `fileN`
  and `bbN` identities but otherwise accepts record fields and unordered
  tables in any order; reprinting restores the canonical order.

Nested `try` regions introduce a new local `bbN` namespace for each of `body`,
`handler`, `orelse`, and `finally`. Ordinary jumps inside a region address that
region. `escape` addresses the enclosing function's block namespace, exactly as
`MirTerm::EscapeJump` does.

## Source Files and Locations

```text
file {
  id: file0,
  path: present("src/main.mojo"),
  module: present("main")
}

loc { file: file0, start: 10, end: 14, origin: present($v0) }
```

Paths and modules may independently be `absent`. Offsets are unsigned UTF-8 byte
offsets into the named source when it is available; source contents are not part
of the artifact. `start <= end` is required. A register location is either
`absent` (generated/no source) or `present(loc {...})`; `(0, 0)` is an ordinary
source range, not a synthetic sentinel. `SyntaxId` is omitted because occurrence
identity has already been resolved before MIR.

## Declarations

### Struct declarations

```text
struct {
  name: Box,
  fields: [field { name: value, type: Int }],
  mut_self_methods: [set],
  fieldwise_init: true,
  param_decls: [],
  explicit_destroy_message: absent,
  explicit_destructors: [],
  explicit_destroy_declared: false,
  explicit_destroy_fields: [],
  conformances: [],
  declared: [],
  associated_types: [],
  parameterized_associated_types: [],
  associated_values: []
}
```

`explicit_destructors` contains `destructor { name: symbol, raises: bool }`
records sorted by name: every named `deinit self` method the struct
declares, whether or not the struct is `Deinitable`. A struct that is not
`Deinitable` carries an explicit-destruction obligation:
`explicit_destroy_message` is `present` with its message (the
`@explicit_destroy` text when `explicit_destroy_declared` is `true`, else
the default message), and `explicit_destroy_fields` lists each direct field
whose type carries its own obligation as `field { name: symbol, struct:
symbol }`, sorted by name. A `Deinitable` struct's message is `absent`, its
`explicit_destroy_declared` `false`, and its `explicit_destroy_fields`
empty. Schema 1.41 added the last two fields and widened
`explicit_destructors` to every struct; a 1.40 or older artifact reads as
undeclared with no obligated field. `associated_types` contains
`associated_type { name: symbol, type: type }` records sorted by name,
`parameterized_associated_types` the minor-46 records sorted by name, and
`associated_values` contains `associated_value { name: symbol, value:
param-expr }` records sorted by name.

A struct that declares a parameterized compile-time value member (`comptime
get[idx: Int]: Int = Self.values[idx]`) carries
`parameterized_associated_values: [parameterized_associated_value { name:
symbol, params: [param-decl...], value: param-expr }, ...]` after
`associated_values` (schema 1.80), sorted by name: upstream's generator,
its body over the struct's binders and its own `params`, every one in
declaration order. The elaborator answers a `param_type_member` read with
arguments (`T.get[0]`) from it, binding the read's arguments to `params`
by position, so elaborated MIR carries none. An older artifact, or a
struct that declares none, omits the field.

A struct that owns a lifecycle member carries `members: [member { name:
symbol, symbol: symbol }, ...]` after `associated_values` (schema 1.65):
its member table, sorted by name, then symbol, naming the function that
is its `__deinit__`, `__copyinit__`, or `__moveinit__`. A concrete struct's
table also names each instance of a member a backend reaches from a
value's type (`mir::DISPATCHED_MEMBERS`: `write_to`, `write_repr_to`,
`write_string`, `__hash__`, `_update_with_simd`, `__eq__`, `__ne__`,
`__int__`, `__setitem__`, `__call__`) and of every other special method
or trait requirement its type reaches
(`mir::MirDeclarations::reached_from_type`), keyed by the template's method
spelling with any overload qualifier (`__eq__$ov$Self`), so a generic or
overloaded member has several entries. A drop, copy, move, display, hash,
or other such dispatch of a value runs the member the table names, never
one composed from the struct's name. MIR lowering records a template's
declared lifecycle members (`S.__deinit__`); the elaborator records each
concrete struct's instances, so an owner-less member shared by every
instance of its struct (`DBox.__deinit__$mono`, `WBox.write_to$mono`)
appears in each instance's table. The concrete verifier reports a table
naming a function the program does not define, a key that is no
lifecycle or dispatched member, or a second entry for a lifecycle member.
The writer omits the field when it is empty, the reader takes an absent
field as empty.

### Function declarations

```text
decl {
  lowered_name: add,
  param_names: [lhs, rhs],
  param_types: [Int, Int],
  defaults: [absent, absent],
  required: [true, true],
  variadic: absent,
  variadic_convention: absent,
  variadic_index: absent,
  kw_variadic: absent,
  kw_variadic_convention: absent,
  kw_variadic_index: absent,
  positional_only: absent,
  keyword_only: absent,
  param_decls: [],
  has_receiver: false,
  receiver_convention: absent,
  param_conventions: [absent, absent],
  return_type: Int,
  returns_reference: false,
  raises: false,
  error_type: absent,
  ref_params: [false, false],
  param_writes: [false, false]
}
```

The parameter names, types, defaults, required mask, conventions, and reference
mask have equal lengths. Variadic conventions are independent of the fixed
parameter list. Indexes use runtime ABI slot numbering. Receiver presence and
convention are separate because a plain receiver has an absent convention.
A nested function's `enclosing` names the declaration it is nested in; the
verifier's binder scope for its body is its own `param_decls` and
`region_binders`, then each enclosing declaration's, then the struct of the
outermost method.

A declaration may end with `inline: always`, `nodebug`, `builtin`, or
`never` after `region_binders` (schema 1.61): the inline level its
`@always_inline`, `@always_inline("nodebug")`, `@always_inline("builtin")`,
or `@no_inline` decorator names, as upstream's function op records it. The
writer omits the field for `automatic`, the level of an undecorated
function, and the reader takes an absent field as `automatic`; any other
spelling is an error. A package record carries the level beside the rest of
the declaration (package format 65).

A declaration may end with `synthesized: true` after `inline` (schema
1.70): the compiler synthesized the function for a struct that declares
none, as upstream synthesizes a `__del__`, `__copyinit__`, and
`__moveinit__` for a struct without one (`S.__deinit__(deinit self)` with
an empty body, `S.__copyinit__(out self, *, copy: Self)` and
`S.__moveinit__(out self, deinit existing: Self)` storing each field of the
source into `self`'s; all `nodebug`). The writer omits the field for a
function the source declares, and the reader takes an absent field as
false. A package record carries the flag beside the rest of the
declaration, and a package unit may be a struct's synthesized destructor
(package format 67), copy constructor, or move constructor (package format
68).

Abstract dispatch requirements (`__trait_dispatch.*`, `__iterator_dispatch.*`) have no concrete declaration record;
a `__trait_dispatch.*` method's parameter contract is its trait requirement's (minor version 48).
Their complete `subscript_call`, `iterator_call`, or stored `func`/`generic_func`
callable contract at the instruction is the declaration of record and must be
verified before runtime retargeting.

## Functions and Blocks

```text
fn {
  name: add,
  registers: 3,
  vars: 2,
  var_names: [lhs, rhs],
  params: 2,
  param_types: [Int, Int],
  owned_params: [false, false],
  deinit_params: [false, false],
  ref_params: [false, false],
  returns_reference: false,
  var_types: [var_type { var: $v0, type: Int },
              var_type { var: $v1, type: Int }],
  return_type: present(Int),
  raises: false,
  error_type: absent,
  register_types: [reg_type { reg: %r0, type: Int }],
  locations: [reg_loc { reg: %r0, location: absent }],
  blocks: [
    bb0 {
      instructions: [var.use { dest: %r0, var: $v0, mode: copy }],
      terminator: return { value: present(%r0) }
    }
  ]
}
```

Counts are explicit and checked against referenced identities. Parameter masks
align with `param_types`. Production artifacts require a present return type.
Register types are explicit and are not re-inferred from mnemonics.

A generic body may carry `removed_params: [param_id { owner: "f", slot: 0 },
...]` between `locations` and `blocks` (schema 1.60): the function's own
compile-time parameters and, for a method, its struct's, or a nested
function's enclosing declarations' parameters, sorted by owner and slot, that the pre-elaboration pass
`remove-unused-params` (upstream's `RemoveUnusedParams`) found its body and
signature never read, which the elaborator leaves out of an instance's
identity. The writer omits the field when it is empty, as it is on every
concrete function, and the reader takes an absent field as empty. The
verifier takes each removed binder out of the body's scope and reports a
removed parameter the body still reads. A package does not encode it: a
package holds its bodies before the pipeline that computes it.

Such a body may also carry `callee_removed_params: [callee_params { callee:
"leaf", params: [param_id { owner: "leaf", slot: 0 }] }, ...]` right after
`removed_params` (schema 1.62): each callee whose removed parameters
`removed_params` discounted a forward to, with those parameters, and each
nested body it names that removed a parameter, sorted by callee. Upstream's
`RemoveUnusedParams` rewrites such a call without the arguments; Mojito
keeps the call, so the verifier reads the body with those arguments, and
the instructions that only computed them, dropped
(`mir::without_removed_forwards`), and a named nested body reading only
what its listed removals keep, before it checks `removed_params`. The
writer omits the field when it is empty, the reader takes an absent field as
empty, and a package does not encode it.

Such a body may also carry `removed_args: [$v0, ...]` right after
`callee_removed_params` (schema 1.64): the parameter slots its body uses
only to drop a value whose drop runs no code, ascending, which
`remove-unused-params` found unused (upstream's `identifyUnusedArguments`).
Its `removed_params` are those the body reads with these arguments gone
(`mir::without_removed_arguments`), and the elaborator instantiates that
reduced body for a direct call, which passes no argument for them, and the
full body, keeping every parameter, for any other use. The verifier reports
a listed slot the body uses otherwise. The writer omits the field when it is
empty, the reader takes an absent field as empty, and a package does not
encode it.

Such a body may also carry `callee_removed_args: [callee_args { callee:
"fa", args: [$v0] }, ...]` right after `removed_args` (schema 1.66): each
callee whose removed arguments the body's `removed_args` and
`removed_params` relied on dropping a forward to, with those arguments,
sorted by callee. Upstream's `RemoveUnusedParams` rewrites such a call to
the clone without them; Mojito keeps the call, so the verifier reads the
body with their operands, and the instructions that only computed them,
dropped (`mir::without_removed_argument_forwards`) before it checks
`removed_args`, and the elaborator builds each instance through the same
view with the operands kept for it to drop
(`mir::with_removed_argument_forwards_erased`). The writer omits the field
when it is empty, the reader takes an absent field as empty, and a package
does not encode it.

## Values, Types, and Semantic Records

Lowercase tags below are reserved canonical spellings. A nullary tag is written
as a bare word; a positional payload uses parentheses; named payloads use a
record.

### Constants

`Const` is one of `int(sint)`, `float(bits_hex)`,
`int_literal(decimal)`, `float_literal(exact)`, `bool(true|false)`,
`string(string)`, `function(symbol)`, `none`, `param(param-expr)` (schema
1.20), a parameter expression read as a runtime value — a pack's
`length`, membership, or conformance — which a generator carries, the
elaborator folds per instance, and concrete MIR never holds, or
`value(ct-value)` (schema 1.27), a closed `ct_simd` or `ct_struct`
parameter value read at runtime. Concrete `float` stores the
exact IEEE-754 binary64 bits as 16 lowercase hex digits. `IntLiteral` uses
arbitrary-precision decimal. `FloatLiteral` uses its canonical exact spelling
and may never round through host `f64`: `-0.0` for negative zero, `{n}.0` for
an integral value, and the reduced `{numer}/{denom}` rational otherwise —
compile-time folding produces exact non-decimal rationals such as `1/3`, so an
exact decimal form does not exist in general. A parser accepts a non-reduced
rational; reprinting reduces it.

`CheckedConst` uses `checked_int`, `checked_float`, `checked_bool`,
`checked_string`, or `checked_none` with the same fidelity rules, plus
`checked_dtype(name)`, `checked_construct { target, arg }` for an
implicit-conversion default, and `checked_evaluate("function")` for a
default lowered as a function. Schema 1.43 adds the applied form
`checked_evaluate { function, arguments: [ty-arg, ...] }`: the function's
own compile-time parameters, in declaration order, written over the
binders of the declaration holding the default, as a conformer's trait
default wrapper reaches its trait member's default. Concrete MIR carries
only the positional form.

`CtValue` uses `ct_int`, `ct_uint`, `ct_float_bits`, `ct_int_literal`,
`ct_float_literal`, `ct_bool`, `ct_string`, `ct_tuple`, `ct_list`, `ct_dict`,
`ct_set`, `ct_dtype`, `ct_simd`, `ct_struct { name, fields }`, `ct_type`,
`ct_reflected`, `ct_trait([trait, ...])` for a trait or a `&` composition of
traits used as a value (schema 1.76), `ct_kgen_string("text")` for a
string literal's KGEN string, its `value` parameter (schema 1.79),
`ct_expr(param-expr)` for a residual
parameter expression,
`ct_deferred(binder)` for a slot whose value arrives later (a callable-value
parameter the VM reifies) and which is no part of generic identity, or
`ct_marker(marker)` for an elaborator classification of a name that is no
parameter: `marker_local`, `marker_type`,
`marker_tuple_origin { id, mutability }`, or `marker_applied`, a module
constant whose initializer applies a callable, which the check binds as its
application (schema 1.38; a 1.37 artifact's `marker_applied(int)` and
`marker_layout(type)` read as the same marker). A deferred slot names the binder
whose slot it fills by a `binder { owner, slot, name }` record.

`ct_pointer { offset: int, slots: dangling | [ct-value | absent, ...] }`
(schema 1.39) carries frozen memory. `absent` is an uninitialized slot;
`dangling` is a pointer without an allocation, distinct from `[]`, a live
zero-length allocation. Offsets count elements and can be negative. The
checked pointer type supplies the element layout when a backend allocates
and writes fresh storage. Nested pointers and strings are recursive values.

`ct_store_to_mem(ct-value)` (schema 1.56) carries a frozen reference into
compile-time memory, upstream's `#lit.store_to_mem`: the value it reads.
A backend thaws it as a fresh compile-time memory slot holding that value;
it has no run-time form.

`ct_removed_arg` (schema 1.63) is a struct argument that mentions a
parameter its instance's generator removed, upstream's `_REMOVED_ARG`. It
stands in a value or a type slot of a concrete struct type
(`Box$mono$V_REMOVED_ARG`), whose declared fields read no erased argument;
the verifier treats that type as agreeing with every instance of the same
template that matches its other arguments. It has no run-time form.

### Parameter expressions

A parameter expression is typed and canonical
(`docs/notes/param-expr-attributes.md`). Operands print in canonical order, a
declared parameter by its owner symbol and slot, a signature binder by depth
and index; no address or process-local key is written.

```text
param_constant(ct-value)
param_decl_ref  { owner: "symbol", slot: N, name: symbol, type: meta }
param_index_ref { depth: N, index: N, type: meta }
param_expr      { op, type: meta, operands: [param-expr...] }
param_identical { left, right }
param_conforms  { subject, trait }
param_trivial   { lifecycle, subject }
param_type_shape(type)
param_select    { elements: [type...], index }
param_list_get  { list, index }
param_list_tabulate { count, element }
param_list_concat { lists: [param-expr...] }
param_field     { base, name: symbol, type: meta }
param_reflect   { subject, query }
param_type_member { subject, name: symbol, args: [param-expr...], type: meta }
param_pack_query { pack, query }
param_apply     { function: "symbol", type: meta, args: [param-expr...] }
```

`param_list_get` is an element of a parameter list that is still a parameter
(a pack element). `param_list_tabulate` and `param_list_concat` (schema 1.29)
are lists computed from packs still open, upstream's `param_list.tabulate`
and `param_list.concat`: the list of `count` elements whose element `i` is
`element` with its index bound to `i`, which `element` names as
`param_index_ref { depth: 0, index: 0 }` of the tabulation's own binder, and
the elements of `lists` in order. `TypeList.reverse()` is a tabulation and
`TypeList._concat[...]()` a concatenation. Such a list is the one argument
of a struct type that spreads it (`Tuple[*Ts.reverse()]`), a
`dependent_parameter` whose expression has the meta-type
`meta_param_list(meta_type)`. A trait-valued parameter (`meta_value(any_trait(...))`)
is the element type of the raw list it bounds (`TypeList`'s `values:
param_list(dependent_parameter(Trait))`), also a `dependent_parameter`. `param_field` is a field of a struct-typed parameter value
that is still a parameter (schema 1.27), `param_reflect` a reflection query over a symbolic subject
(`is_struct()`, `field_count()`, `field_names()`, `field_types()`,
`field_index["name"]()`, `field["name"].T`), `param_type_member` a
compile-time value member of a symbolic subject type (`T.K`, schema 1.36),
applied to `args` when the member is parameterized (`T.get[0]`; schema
1.80, written only when non-empty), and `param_apply` a compile-time
application of a callable symbol, never folded (schema 1.14;
`docs/notes/param-expr-attributes.md` §Register types). Only the elaborator
below MIR evaluates one; a 1.37 artifact's `evaluated` field is read and
dropped (schema 1.38).

`op` is one of `add`, `mul`, `neg`, `sub`, `div`, `floordiv`, `mod`, `pow`,
`shl`, `shr`, `and`, `or`, `xor`, `eq`, `lt`, `le`, `bool_and`, `bool_or`,
`bool_xor`, `cond`. `meta` is `meta_value(type)`, `meta_type`,
`meta_reflected`, `meta_tuple([meta...])`, `meta_list([meta...])`,
`meta_set([meta...])`, `meta_dict([meta_entry { key, value }...])`, or
`meta_param_list(meta)`. A pack query's `pack` is the
`binder { owner, slot, name }` record of the pack binder it queries, and its
`query` is `pack_length`,
`pack_conforms(symbol)`, `pack_predicate { predicate, all }`,
`pack_contains(param-expr)`, or `pack_index_of(param-expr)` (schema 1.31:
upstream's `_get_type_index[element, *Ts]()`, the first position of the
element type in the pack, else `-1`).

Parsing re-enters the canonicalizing constructors, so a parsed expression is
canonical whatever order the text spelled its operands in, a bad arity or
operand domain is a diagnostic at the expression, and a `param_expr`'s recorded
`type` must be the type it builds to. An unknown or unbound parameter
(`param_hole`) never crosses into MIR and is rejected; every other form is
read back, a pack element and a reflection query included.

### Types

The complete `Ty` tag set is:

```text
Int UInt Bool StringLiteral Float64 None Never IntLiteral FloatLiteral Infer
DType Self Error
func { environment, params, names, return_type, required, variadic,
       kw_variadic, positional_only, keyword_only, raises, error_type,
       conventions, ref_params, ref_return, transfers }
generic_func { environment, param_decls, params, names, return_type, required,
               variadic, kw_variadic, positional_only, keyword_only, raises,
               error_type, conventions, ref_params, ref_return, transfers }
overload([type...])
param { owner, slot, name, bounds, callable_bound }
assoc { base, member, arguments }
dependent_parameter(param-expr)
struct_type { name, arguments }
simd { dtype, width }
comptime_list(type) param_list(type) tuple([type...]) runtime_pack([type...])
variadic_pack(type) variant([type...])
pointer { element, origin }
ref { referent, origin, mutability }
```

`TyArg` tags are `type_arg`, `value_arg`, and `origin_arg`. DTypes and all AST
operators/conventions use their lowercase source-independent enum names;
conventions are `read`, `var`, `mut`, `out`, `ref`, and `deinit`. A `simd`
slot is a dtype or an integer, or `ct_expr(<param-expr>)` for a lane still
symbolic; the verifier keeps a symbolic slot out of every artifact, so that
form only serves lossless round trips, and a closed expression folds back to
the canonical concrete type on read.

`param_list(type)` (schema 1.75) is upstream's raw parameter list
`!kgen.param_list<type>`, the type of `ParameterList`'s `values` parameter. It
types compile-time parameter declarations only: elaborated MIR rejects a
register, slot, or signature that holds one.

`any_trait([trait, ...])` (schema 1.76) is upstream's trait metatype
`AnyTraitType`, spelled `type_of(Trait)` in source: the type of a trait used
as a value, over its canonical trait set. Like `param_list`, it types
compile-time parameter declarations only.

`TypeType` (schema 1.77) is upstream's KGEN metatype `!kgen.type`
(`_MLIR.KGENTypeType`), the type of every type and trait, which the pin prints
`__TypeOfAllTypes`. It is compile-time only, as `any_trait` is; a type binder
it types records the bound `__TypeOfAllTypes`.

`KgenString` (schema 1.79) is upstream's KGEN string type `!kgen.string`, the
type of a `StringLiteral`'s `value` parameter, whose values are
`ct_kgen_string`. It is compile-time only, as `any_trait` is.

`trait_type([trait, ...])` (schema 1.78) is upstream's `TraitType`: a trait,
or a composition, used as a type, the type of every type conforming to its
canonical trait set. It is a raw list's element (`param_list(trait_type([AnyType]))`
is `!kgen.param_list<AnyType>`, a list of types) and is compile-time only, as
`any_trait` is.

`ParamDecl` is
`type_param { owner, slot, name, bounds, trait_param, callable_bound, default, infer_only, variadic, constraints }`
or
`value_param { owner, slot, name, type, default, callable_default, infer_only, variadic, constraints }`.
Callable defaults use `default_symbol`, `default_parameter`, or
`default_if { condition, then_value, else_value }`. A type parameter's
`trait_param` (schema 1.78; `absent` when an older record omits it) is
`present(binder { owner, slot, name })` when a trait-valued sibling bounds it
(`T: Tr`, upstream's `ParamType`), its `bounds` then that sibling's declared
trait set.

#### Binder identity

A binder's `owner` (a quoted declaration symbol) and `slot` (its position in
the declaration's parameter list) are its identity, exactly as in
`param_decl_ref`; `name` is its source spelling and carries no identity. A
`param` type names the binder it uses by the same two fields, so a use and its
declaration agree by identity, and two declarations that both spell `T` stay
apart. In a 1.1 artifact, whose binders carry only a spelling, every binder
spelled `n` reads as one identity per spelling (owner `$mir-1.1:n`, slot `0`),
which is what a 1.1 artifact meant. A `type.construct` without `owner` and
`slot` constructs the binder its destination register is typed by, when that
binder carries the spelling, and reads as a 1.1 binder otherwise.

A `where` operand or a callable default names its binder by a
`binder { owner, slot, name }` record. An operand no declaration binds — an
origin's mutability parameter, which is erased from its declaration's
parameter list, or an associated member's own parameter — is an unbound
reference: owner `$unbound:<name>`, slot `0`, identified by its spelling.

`GenericConstraint` is a prefix tree. Its tags map one-to-one to the public
variants: `with_message { condition, message }`, `conforms`, `conforms_pack`,
`pack_predicate` (whose predicate is `predicate_trivial` or
`predicate_alias`), `pack_contains`, `trivial`, `eq`, `ne`, `lt`, `le`, `gt`,
`ge`, `and`, `or`, `not`, `constraint_bool`. Constraint operands are
`operand_param`, `operand_value`, `operand_type`, `operand_pack_length`, and
`operand_expr(param-expr)` for an arithmetic operand. A value parameter's
`default` and a `default_if` condition are parameter expressions.

### Schema 1.0

A 1.0 artifact spells a parameter expression as a name-only tree —
`ct_param(symbol)`, `ct_value(ct-value)`, `ct_neg`, and `ct_add`, `ct_sub`,
`ct_mul`, `ct_floor_div`, `ct_mod`, `ct_pow` as `{ left, right }` records — a
finite type selection as `dependent_index { elements, index }`, and both a
parameter reference and a deferred slot in value position as `ct_param`. The
1.1 reader translates these into the canonical graph and the writer never
emits them; in a 1.1 artifact they are errors.

A 1.0 name carries no type, so it resolves through the value parameters the
artifact declares (`value_param { name, type, ... }`, callable-valued ones
excluded). Exactly one declared type gives a typed reference. None gives a
deferred slot in value position, read as an unbound reference of that
spelling, and an error in expression position. Several
different declared types are an error that asks for a 1.1 re-emission: nothing
in a 1.0 reference chooses between them, and the reader does not guess.

### Origins and callable environments

Origin path segments are `field(symbol)`, `any_index`, `interior(symbol)`, and
`subtree`. Origins are `origin_param(id)`, `origin_self`,
`origin_place { root: $vN, path }`, `origin_union`, `origin_static`, and
`origin_untracked { mutable }`. Pointer origins are `pointer_place`,
`pointer_param`, `pointer_self`, `pointer_static`, `pointer_untracked`, and
`pointer_unsafe_any`; all fields of the corresponding `PointerOrigin` variant
are required.

Mutability is `immutable`, `mutable`, or `mutability_param(id)`. Signature
origins are `sig_self`, `sig_param(index)`, `sig_bound(origin)`, `sig_static`,
`sig_untracked`, `sig_unsafe_any`, `sig_projected`, `sig_union`, `sig_infer`,
and `sig_carried(type)`. Signature
mutability is `sig_immutable`, `sig_mutable`, `sig_bool_param(index)`, or
`sig_infer`. A `RefSig` is `ref_sig { origin, mutability }`.

Capture access is `read` or `write`. Capture sets are `capture_set_infer`,
`capture_set_param(id)`, or `capture_set([capture_origin...])`. Callable
environments are `default`, `thin`, or `capturing(capture_set)`.

Every transfer, call argument, boundary, result adapter, iterator call, generic
instantiation, subscript call, closure capture, and capture access is a tagged
record containing all fields of its same-named checked/MIR structure in the
public field order. Enum variants use snake-case tags. No source AST expression,
span-keyed lookup, or inferred default is permitted in these records.

## Places, Loans, and Interior Metadata

```text
place {
  root: $v0,
  root_type: present(Box),
  projections: [
    projection { op: field(value), type: Int }
  ],
  type: present(Int),
  through: present($v1)
}

loan {
  place: place {...},
  mutable: false,
  interior: present(interior_origin {
    root: $v0,
    path: [interior(element)]
  }),
  shared: false,
  setup: false
}
```

Projection operations are `field(symbol)`, `index(%rN)`, `const_index(uint)`,
`variant(variant-index)`, `uninit_payload`, and `rebind` (schema 1.37), whose
type is the rebind's target. A `variant-index` (schema
1.31) is a known alternative's `uint`, or `ct_expr(param-expr)` in a
template: upstream's `_get_type_index[T, *Ts]()`, spelled
`param_pack_query { pack, query: pack_index_of(element) }`, which
elaboration closes per instance. The `index`, `input_index`, and
`output_index` fields of the `variant.*` instructions are `variant-index`
values too; concrete MIR holds only the `uint` form. Each projection pairs with its resulting
type, preserving `projection_tys`. Root and terminal types retain their explicit
optionality for compatibility MIR, although verified production artifacts
require them.

`MirPlace::through`, loan mutability, the shared flag of a `Pointer(to=place)`
alias loan, the setup flag of a call operand awaiting its read, interior roots/paths, destination domains, invalidation exceptions,
and capture accesses are semantic data even when the VM erases them. Verification must prove that a through slot is a compatible
reference capability, mutable loans do not recover unavailable permission, and
canonical interior identities agree with their executable place/reference
origin relationship.

## Instructions

Every instruction is `mnemonic { field: value, ... }`. Field names and order are
the `MirInstr` variant's public fields in `src/mir/ir.rs`; their values use the
schema types above. This table is exhaustive and freezes the variant mapping:

| MIR variant | Mnemonic |
|---|---|
| `EstablishLoans` | `loans.establish` |
| `InvalidateInteriors` | `interiors.invalidate` (pre-drop and drop-elaborated only: `lower-lit` erases it before the pre-elaboration pipeline, and concrete verification refuses one) |
| `MakeRef` / `ReadRef` / `WriteRef` | `ref.make` / `ref.read` / `ref.write` |
| `CopyValue` / `Rebind` | `value.copy` / `value.rebind` — a `value.copy` carries `constructor: "<symbol>"` after its operands when it names the `__copyinit__` it runs, as `var.use` does (schema 1.68) |
| `MakeClosure` / `KeepAlive` | `closure.make` / `lifetime.keep_alive` (`lifetime.keep_alive` as `interiors.invalidate`: `lower-lit` erases it; schema 1.71: a `closure_capture` of mode `copy` or `move` carries `constructor: "<symbol>"` after its `mode` when it names the `__copyinit__` or `__moveinit__` it runs, as `var.use` does; schema 1.73: an owned capture carries `destructor: "<symbol>"` after its `constructor` when it names the `__deinit__` its value runs at the closure's death, as `drop.var` does) |
| `Const` / `MaterializeLiteral` / `SizeOf` / `TypeName` | `const` / `literal.materialize` / `layout.size_of` / `type.name` |
| `ConstructType` | `type.construct_expr` (schema 1.33: `{ dest, type }`) |
| `ParamListAddress` | `param_list.address` (schema 1.34: `{ dest, values }`, `values` a parameter or closed-tuple constant) |
| `StoreToMem` | `store_to_mem { dest, src }` (schema 1.56): moves `src` into compile-time memory, which is never destroyed, and sets `dest` to an immutable reference to it — upstream's `store_to_mem`. Only a lifted compile-time evaluation's body holds one |
| `UseVar` / `DefVar` | `var.use` / `var.store` (schema 1.49: `var.store` adds `in_place`, true when the slot is the preceding non-raising call's result storage; schema 1.68: a copying or moving `var.use` carries `constructor: "<symbol>"` after its `mode` when it names the `__copyinit__` or `__moveinit__` it runs, as upstream's copy/move insertion names the constructor at each site: the template member in a parametric body, its instance in concrete MIR; the writer omits the field when the use names none, the reader takes an absent field as none, and the verifier reports a use naming an undeclared function, a borrow or forward naming any, or a copy or move naming other than the constructor its struct's member table records) |
| `MovePlace` / `LoadPlace` | `place.move` / `place.load` |
| `UnOp` / `BinOp` | `unary` / `binary` (schema 1.53: both add `raises: option<type>`, the error type of the operator method's call, as a call's `raises`) |
| `Call` / `CallIndirect` / `MethodCall` | `call` / `call.indirect` / `call.method` (schema 1.71: a `call.method` with a `result_adapter` carries `adapter_constructor: "<symbol>"` after its operands when the adapter's copy of the referent names the `__copyinit__` it runs, as `value.copy` does) |
| `PointerStorageTake` / `PointerStorageDestroy` | `pointer.take` / `pointer.destroy` — a `pointer.destroy` carries `destructor: "<symbol>"` after `element` when it names the element's `__deinit__`, as `drop.var` does (schema 1.73) |
| `UninitStorage` / `UninitStorageTake` / `UninitStorageDestroy` | `uninit.make` / `uninit.take` / `uninit.destroy` — an `uninit.destroy` carries `destructor: "<symbol>"` after `element` when it names the payload's `__deinit__`, as `drop.var` does (schema 1.73) |
| `MarkInitialized` | `ownership.mark_initialized` (schema 1.28: `{ place }`, upstream's `lit.ownership.mark_initialized`) |
| `MarkDestroyed` | `ownership.mark_destroyed` (schema 1.30: `{ place }`, upstream's `lit.ownership.mark_destroyed`: the place holds no value from here and no destructor runs on it) |
| `GetField` | `field.get` — a slice descriptor's bound read (`s.start`) carries `constructors: ["<symbol>", …]` after its `field` naming the `Optional` constructors it builds its result through, the nullary one and the positional one-argument one, as an upstream call names the constructor it runs (schema 1.69); the elaborator names them in concrete MIR, the writer omits the field when the read names none, and the reader takes an absent field as none |
| `Index` / `Slice` / `MultiIndex` / `MultiSet` | `index.get` / `slice.get` / `index.multi` / `index.multi_set` |
| `Store` / `StoreRef` | `place.store` / `place.store_ref` |
| `MakeTuple` | `tuple.make` — one whose `dest` register is typed `$variadic[T]` is the homogeneous collector an inlined call builds for its callee's `*xs: T`, and one typed a tuple with `element_types` the elements a heterogeneous collector's or a typed `Tuple[T, ...]` construction's (no schema change) |
| `MakeVariant` / `VariantIs` / `VariantGet` / `VariantSet` | `variant.make` / `variant.is` / `variant.get` / `variant.set` (schema 1.71: `variant.make` and `variant.set` carry `constructor: "<symbol>"` after their operands when the payload copy names the `__copyinit__` it runs, as `value.copy` does; schema 1.73: `variant.set` carries `destructors: [present("<symbol>") | absent, ...]`, one entry per alternative, naming the `__deinit__` the replaced payload runs under that tag, omitted while it names none) |
| `VariantTake` / `VariantSetInitWith` / `VariantDeinitWith` / `VariantReplace` | `variant.take` / `variant.set_init_with` / `variant.deinit_with` / `variant.replace` (schema 1.71: `variant.replace` carries `constructor: "<symbol>"` as `variant.make` does; schema 1.73: `variant.set_init_with` carries `destructors` as `variant.set` does) |
| `MakeSimd` / `SimdCast` / `SimdBitcast` / `SimdShuffle` | `simd.make` / `simd.cast` / `simd.bits` / `simd.shuffle` — `dtype` is a dtype name or `ct_expr(...)`, `width` a lane count or `ct_expr(...)` (schema 1.18); a shuffle's `mask` is a list of lane indices, or a template's `lane_shuffle { lanes: [<param_expr>, ...] }`, `lane_slice { offset: <param_expr>, width: <width> }`, or `lane_join {}`, which elaboration closes (schema 1.26) |
| `Raise` / `Try` | `raise` / `try` |
| `DeclareVar` | `var.declare` (schema 1.50: a `var x: T` slot's declaration point; no def, no use, a no-op on both backends, counted as a def site by region locality; unlike the lifetime markers it survives `lower-lit`, as upstream lowers `lit.var.decl` to an allocation) |
| `Drop` / `DropVar` / `DropPlace` | `drop.reg` / `drop.var { var }` / `drop.place { place }` — a `drop.var` or `drop.place` carries `destructor: "<symbol>"` after its operand when it names the `__deinit__` it runs on the whole value, as upstream's destructor insertion names the `__del__` at each death: the template member in a parametric body, its instance in concrete MIR; the writer omits the field when the drop names none, the reader takes an absent field as none, and the verifier reports a drop naming an undeclared function or other than the destructor its struct's member table records (schema 1.67) |
| `ConsumeVar` / `ConsumePlace` | `consume.var` / `consume.place` |
| `Unsupported` | `unsupported { message: string }` |
| `If` | `if { results: [reg...], condition: %rN, then: [block...], elifs: [elif { condition: [block...], then: [block...] }...], else: [block...] }` — `hlcf.if` (schema 1.54) |
| `Loop` | `loop { label: option<loopN>, body: [block...] }` — `hlcf.loop` (schema 1.54); `loop4294967295` is upstream's `inlined_cf_scope`, the label `inline-parametric` wraps an inlined body in, which no source loop carries |
| `Match` | `match { cases: [[block...]...], else: [block...] }` — `hlcf.match`: the cases in order, each a region that tests its pattern and runs its body, leaving by `match.next` (the next case, or `else` after the last) or `match.complete` (the op's end); `else` runs when no case completed and ends in `yield` with no values (schema 1.74) |
| `ComptimeIf` | `comptime.if { results: [reg...], conditions: [<constraint>...], arms: [[block...]...], else: [block...] }` — `hlcf.comptime.if`, one arm per condition (schema 1.54) |
| `ComptimeFor` | `comptime.for { binder: <binder>, initial: <param_expr>, has_next: <param_expr>, next_iter: <param_expr>, next_value: <param_expr>, body: [block...], else: [block...] }` — `hlcf.comptime.for`; the four sequence fields are the `ComptimeFor` terminator's; `else` is one block ending in `unreachable {}`; `body` is one block ending in `unreachable {}` that holds a `comptime.if` on `eq { left: operand_expr(<has_next>), right: operand_value(ct_bool(true)) }`, whose arm is the suite and whose else arm is the stop arm, ending in `comptime.for.break {}`, as `LowerSemanticCF` leaves upstream's body (schema 1.54; the `slot` field and the `elements` source withdrawn, schema 1.57; the body's `has_next` test, schema 1.58) |
| `ComptimeAssert` | `comptime.assert { condition: <constraint> }` — upstream's `kgen.param.assert`, a `comptime assert` over the function's binders, its message (if any) a `with_message` around the condition; the elaborator judges it per reached instance, failing the instance `constraint failed: MESSAGE`, and concrete MIR carries none (schema 1.81) |
| `GetIter` / `HasNext` / `Next` / `TryNext` | `iter.init` / `iter.has_next` / `iter.next` / `iter.try_next` (schema 1.71: an `iter.try_next` whose `call` has a `result_adapter` carries `adapter_constructor: "<symbol>"` after its operands, as `call.method` does) |

`UseMode` is `copy`, `move`, `borrow_shared`, `borrow_mut`, or `forward` (schema 1.51: a whole-slot move an owned parameter takes, which runs no move initializer). Intrinsic
subscripts are `tuple_storage`, `variadic_storage`, `simd`, `pointer`, and
`comptime_list`. Slice descriptors are `slice`, `contiguous_slice`, and
`strided_slice`. Result adapters currently contain only
`copy_iterator_reference`.

For `Try`, `body`, `handler`, `orelse`, and `finalbody` contain lists of local
blocks, as every region of `if`, `loop`, `comptime.if`, and `comptime.for`
does: a region's blocks are numbered `bb0..bbN` among themselves, entered at
`bb0`. Parametric MIR, and so every artifact `emit-mir` writes, holds
them, one block per region; concrete MIR carries none, since the
elaborator runs the `lower-control-flow` pass on each template it fetches. Handler absence differs from a present empty handler. Its `cleanup`
runs on the body's raise edge only, before the handler or the `finally`, and
names the slots some raise in the body still holds (schema 1.53; before, it
ran on every exit of the body). Each entry is a `drop { var: $vN,
destructor: "<symbol>" }` record naming the `__deinit__` the drop runs, as
`drop.var` does, `destructor` omitted when it names none (schema 1.72;
before, a list held bare slots, which the reader still accepts as drops
naming no destructor). A list naming a slot every raise path has
already ended fails verification at load. All call fields,
including caller places, capture accesses, compile-time arguments, instantiated
contracts, reference-result ABI, and checked subscript contracts, are required
as explicit options/lists. Backends must not reconstruct omitted selections.

## Terminators

| MIR variant | Canonical record |
|---|---|
| `Jump(target)` | `jump { target: bbN }` |
| `Branch` | `branch { condition: %rN, then: bbN, else: bbN }` |
| `ComptimeBranch` | `comptime_branch { condition: <constraint>, then: bbN, else: bbN }` — a `comptime if` on a parameter expression over the function's binders (the `availability` constraint grammar), decided by the elaborator; concrete MIR carries none |
| `ComptimeFor` | `comptime_for.iterator { binder: <binder>, initial: <param_expr>, has_next: <param_expr>, next_iter: <param_expr>, next_value: <param_expr>, body: bbN, exit: bbN }` — a `comptime for` header by upstream's iterator protocol: the binder `binder` is the iterator, starting at `initial` (the application of the iterable's lifted `__iter__()` call) and replaced by `next_iter` while `has_next` holds, both applications of the `paramfor_*` stubs over the binder; the loop variable is an alias of `next_value`, the `paramfor_next_value` application over it, and has no slot; the body's back edge jumps to the header, and its stop arm, as a `break` does, to `exit`; the elaborator unrolls it, and concrete MIR carries none (schema 1.16; the `iterator` form, schema 1.52; its `next_value`, and the withdrawal of the `range` form, schema 1.55; the withdrawal of the `elements` form `comptime_for.elements { …, elements, … }` and of the header's `slot` field, which the reader now rejects, schema 1.57) |
| `Return` | `return { value: option<reg> }` |
| `ReturnWithCleanup` | `return.cleanup { value: option<reg>, cleanup: [drop { var: $vN, destructor: "<symbol>" }...] }` — drop elaboration writes it on a return that crosses a `finally`: `cleanup` names what the return still holds after the last `finally` it crosses, and runs there (schema 1.53; before, it named loop-owned values); each entry names its destructor as a `try`'s does (schema 1.72) |
| `TryYield` | `try.yield {}` — `lit.try.yield`: completes a `try` part, which goes on to its `else`/`finally` (schema 1.59; before, `falloff {}`, which the reader now rejects) |
| `EscapeJump` | `escape { target: bbN, cleanup: [drop { var: $vN, destructor: "<symbol>" }...] }` — `cleanup` runs at the escape, before the `finally` blocks it crosses; each entry names its destructor as a `try`'s does (schema 1.72) |
| `Yield` | `yield { values: [reg...] }` — `hlcf.yield`: ends an `if` arm, its values defining the op's results (schema 1.54) |
| `ElifCond` | `if.elifcond.yield { condition: %rN }` — ends an elif condition region with its `Bool` (schema 1.54) |
| `Break` / `Continue` | `break { label: option<loopN> }` / `continue { label: option<loopN> }` — `hlcf.break`/`hlcf.continue`: the nearest enclosing `loop`, or the nearest carrying the label (schema 1.54) |
| `ComptimeYield` | `comptime.yield { values: [reg...] }` — ends a `comptime.if` arm (schema 1.54) |
| `ComptimeBreak` / `ComptimeContinue` | `comptime.for.break {}` / `comptime.for.continue {}` — the nearest enclosing `comptime.for` (schema 1.54) |
| `MatchNext` / `MatchComplete` | `match.next {}` / `match.complete {}` — `hlcf.match.next`/`hlcf.match.complete`: leave the nearest enclosing `match` case for the next case or for the op's end (schema 1.74) |
| `Unreachable` | `unreachable {}` — `hlcf.unreachable` (schema 1.54) |

The structured terminators end only the region of their op; the verifier
rejects one anywhere else. Concrete MIR carries none.

Unknown terminators and instructions are fatal for schema major version 1.

## Complete Artifact Example

```text
mojito-mir 1.51
artifact {
  features: [],
  files: [file { id: file0, path: present("main.mojo"), module: absent }],
  structs: [],
  decls: [decl {
    lowered_name: identity, param_names: [value], param_types: [Int],
    defaults: [absent], required: [true], variadic: absent,
    variadic_convention: absent, variadic_index: absent, kw_variadic: absent,
    kw_variadic_convention: absent, kw_variadic_index: absent,
    positional_only: absent, keyword_only: absent, param_decls: [],
    has_receiver: false, receiver_convention: absent,
    param_conventions: [absent], return_type: Int,
    returns_reference: false, raises: false, error_type: absent,
    ref_params: [false], param_writes: [false]
  }],
  functions: [fn {
    name: identity, registers: 1, vars: 1, var_names: [value], params: 1,
    param_types: [Int], owned_params: [false], deinit_params: [false],
    ref_params: [false], returns_reference: false,
    var_types: [var_type { var: $v0, type: Int }],
    return_type: present(Int), raises: false, error_type: absent,
    register_types: [reg_type { reg: %r0, type: Int }],
    locations: [reg_loc { reg: %r0, location: present(loc {
      file: file0, start: 35, end: 40, origin: present($v0)
    }) }],
    blocks: [bb0 {
      instructions: [var.use { dest: %r0, var: $v0, mode: copy }],
      terminator: return { value: present(%r0) }
    }]
  }]
}
```

## Package Container

A compiled package (`mir::text::package`) holds one checked record per
lowering unit, one record per library declaration, and one module table
holding each module's link record, one file per package. The bundled library's is the
only producer today (`src/package.rs`). No part of a package is MIR text:
it is a header followed by bytecode records, as upstream's package is
MLIR bytecode throughout. The format line is `mojito-package 77`.

```text
mojito-package 77
<the header's length, 8 bytes little-endian>
<the header, as bytecode>
<the payload: every record, as bytecode, then the type section>
```

- The header (`PackageDirectory`, `postcard`) holds the package's identity
  (`package`, `compiler`; no library root, since the package names each
  file relative to its library's root and the reader joins its own,
  `mojito_checked::package::LibraryRoot`), the payload's length, and the payload
  ranges of the declaration directory and the type section. A reader refuses a header whose ranges leave the payload or
  whose payload length differs.
- The type section (`mojito_types::type_section`, format 29) holds every
  type the records hold once, as MLIR bytecode's attribute and type
  section does: a `u32` entry count, each entry's start and the last
  one's end as little-endian `u32` offsets, then the entries. An entry is
  `postcard` of the type's variant (`TyKind`) with each child type the
  `postcard` varint index of an earlier entry, and every record refers to
  a type by its index the same way. The writer keys an entry by its bytes,
  so equal types with different decorations (a callable's transfer
  effects, a display's spelling) keep separate entries. A reader refuses a
  section whose offsets are malformed, and decodes an entry the first time
  a record refers to it, sharing its node with every later reference.
- The declaration directory (`mojito_checked::package::DeclarationDirectory`)
  is read in place. Every name it holds is written once, in one string
  section, as an MLIR bytecode string section is: a LEB128 count, each
  string's LEB128 length in reverse order, then the strings' bytes. Ten
  tables follow, each table's row count first, then each table's rows of
  little-endian `u32` words, a name being its string's index:
  - the module table: each library module the build's link recorded,
    sorted by dotted name, with its source file's canonical path and the
    path its declarations are stamped with, each relative to the library's
    root, its link record's range, and the
    span of its module bindings, as upstream's package holds one module
    tree its importer's link and front end both read;
  - each library declaration, sorted by linked name, with its module's
    position, its record's range and residue offset, and the span of its
    checker table keys;
  - every struct declaration, sorted by name (the package order), as its
    declaration's position and its index in that record;
  - one row per function, in the order the bodies were lowered in, which
    `emit-mir` and the elaborator's candidate order observe: its name, its
    declaration's position and its index in that record, its runtime
    parameter count, its `ref_params` mask's span, and the range of the
    unit it lowers from (several functions may share one), as an MLIR
    function op names its symbol once and holds its region, so
    overload resolution and the loan analysis read a function's signature
    without decoding anything; and a by-name permutation of those rows
    (the package order of function declarations);
  - each declaration's keys, every checker table key sorted by table then
    key beside its declaration (the `Traits` range is the package's sorted
    trait list), every method by name beside each declaration whose
    struct declares it, each module's bindings sorted by name, and the
    mask words, 32 parameter slots a word.

  The build writes every table sorted as its lookups need, refusing a
  name two declarations hold, a body no declaration declares, and a
  declared function with no body. A reader builds only the strings'
  offsets, refusing a directory whose strings or tables overrun it; a
  lookup binary-searches the mapped rows, and a malformed row makes a
  lookup miss, never read outside the payload. A build also refuses a
  declaration whose module the link recorded no row for.
- A declaration's record is the declaration once, read by every phase. Its
  lowered part is `postcard` of `LoweredDeclaration`: every
  `MirStructDeclaration` and `MirFunctionDeclaration` the declaration
  lowers to (its own, its methods, and the functions nested in or lifted
  from it), its trait with the traits it refines, and the transfer and call-through effect summaries of its
  callables, each by its function's index in the record, and the integer
  value of the module constant the declaration is (`constant`), named by
  the declaration's row (format 23); it stores no overload set, which an importer finds by
  looking up the declarations under a name (format 20). Every list is sorted by
  name and every set and map written sorted, so equal declarations encode
  equal bytes. It names nothing the directory holds (format 21), as an MLIR
  bytecode op names its symbol by an index into the one string section:
  each function is its row in the function table (`function_rows`), whose
  name the decoder joins back, after checking the row places the function
  at its index in the record; its struct and trait are the declaration's
  own, named by the declaration's row; and a function's `ref_params` is
  derived from its `param_conventions`. The build refuses a record whose
  masks or names would not join back. The
  importer decodes it once per process, the first time any phase looks one
  of its names up (`PackageDeclarations`), as upstream's importer resolves a
  declaration from the package's bytecode on first lookup; one that does
  not decode is reported and refuses the elaboration. Its checker residue
  follows, opaque to the container: the checker's rows for the declaration
  (`checker::interface::DeclarationInterface`), each table sorted by key,
  less what the lowered part says. A keyed table is written as its values
  alone, and a projected struct or binding without its name: the keys are
  the declaration's span of directory keys, the projected structs are the
  `Structs` keys with a lowered struct, the projected bindings the
  `Bindings` keys with a lowered function of their name, and a projected
  struct's `ExplicitDestroy` key holds no row. A projected struct's method
  residue names its method by its function's index in the record, and the
  join splits that function's name back into the method's name and
  overload qualifier (format 23). The build refuses a residue
  whose names are not those its keys and lowered part imply, so the join
  restores each name from the string section. A struct's declared parameters, fields,
  fieldwise initializer, destructors, and explicit-destruction obligation,
  each method's signature apart from what MIR does not carry (its
  reference signatures, view-return and origin facts, and overload
  qualifier), its default attributes included, and a `def`'s type apart from its callable
  environment, reference signatures, and transfers (when they are not its
  summary's) are the lowered part's; the package build refuses the
  package when one says other than the check, byte for byte. A residue
  holds no binding owner, span-keyed fact, syntax identity, or parameter
  hole, and a trait requirement holds each default as its attribute (a
  literal constant, or a parameter expression over the method's value
  parameters, format 51), never as syntax; its expressions decode stamped with the path the importer reached
  the module by, so a compilation decodes it, and joins it with the
  lowered part, the first time a lookup names one of its keys. One that
  does not decode fails the compilation. A module constant's residue
  holds the constant as the build's comptime elaboration resolved it, and
  a declaration's residue holds the module-scope compile-time facts its
  names bind (tables `Elaborated`, `ComptimeDtypes`, `ComptimeParameters`,
  `ComptimeValues`, `TypeAliases`, and `SimdLengthBinders`, the last keyed
  by the binders' owner); no module has a checker record of its own.
- The importer maps the file (`PackageBytes`, `memmap2`) and decodes only
  the header eagerly. A cached package is written to
  a private partial file and renamed over its path, and removed only by
  unlinking, so a mapping's bytes never change under it. Link records are
  copied out of the mapping, once per process, since the linker that
  reads them sits below the package's declarations. The package holds
  no interface of its own: every program imports it (format 28 dropped
  the list of its top-level names a program once had to avoid).
- A unit's record is `postcard` of its file table (each file id's optional
  path, relative to the library's root; every node's module or source,
  and every checked site's and default wrapper's module, is a file id)
  and of its `checked_unit::CheckedUnit`. That holds the lowering unit's
  kind (a struct method's with its position and the symbol it lowers
  to), a
  wrapper's or trait default's declaration less its body, and the unit's
  `CheckedTables` (format 37): its checked expression and statement
  nodes and its declarations, each node holding its own op, type, and
  attributes as the check left them, as upstream's body is typed ops; its
  root statements and a wrapper's; and each location a spelled sugar was
  written at, beside the node of the construction the check spelled for
  it. The nodes are a module `def`, a trait default's declaration, a
  module constant, or a struct shell holding only its name and the one
  method (none for a wrapper). A node's identity is its position, given
  back on decode. Each expression, statement, and declaration node
  encodes as an MLIR bytecode op does behind its `OpEncodingMask`
  (`checked::encoding`, format 50): an encoding mask (a `u16` varint),
  then its always-present parts, then only the attributes the mask
  names, in the mask's bit order; an absent attribute (`None`, empty, or
  default effects) writes nothing. An expression's parts are its op,
  span, source, and syntax id; mask bits 0–1 hold its value category,
  and bits 2–10 name `ty`, `place_ty`, `binding`, `adjustments`,
  `binding_ty`, `effects`, `contextual_base`, `comprehension_bindings`,
  and `call_transfers`, most frequent first, so a common node's mask is
  one byte. A statement's parts are its op, span, module, and syntax id,
  bits 0–2 naming `binding`, `declaration`, and `binding_ty`; a
  declaration's are its kind, name, and location, bits 0–2 naming
  `binding`, `children`, and `ty`. No statement or expression is encoded
  as syntax: a declaration op encodes its symbol, its registered name for
  a method, and its checked signature (its compile-time parameters, each
  declared parameter's type, its result type, and its effect), a `def` op
  its explicit captures, a local `comptime` op the application it
  denotes, if any (format 52), and a struct shell its fields' checked types. An
  op's type operand encodes as the attribute the check resolved, a type,
  value, or origin. The facts of
  declaration sites (checked annotation types, compile-time parameter
  lists, declaration effects) are written as site-keyed rows beside
  them; a struct's own sites are left to its declaration record, and the
  importer gives a member's unit its struct's parameter list from there.
  It is written under `token::encoding_occurrences`, where a `SyntaxId`
  encodes as a number dense within the unit, in the order the encoding
  first meets it, or as its derivation, and a source path as its file
  id, so a site row keeps the occurrence it is keyed by; it is read under
  `token::decoding_occurrences`, which draws one fresh identity per number
  and derives a derived one from its decoded parent. Outside those scopes
  an AST node encodes no identity or path, as a link record's and a
  residue's do, and a `SourceSpan` does not encode. The package build's
  binding owners the nodes name are carried as bare numbers, under
  `origin::encoding_packaged_owners` (R561, R611). The importer lowers a
  unit once per process when the elaborator first reaches one of its
  functions (`PackageBodies`): it installs the unit's tables as its
  checked program over the package's own overload sets and constants
  (`CheckedProgram::installed`, with no arena rebuilt), lowers it to MIR,
  closes its register types against the package's declarations, then
  inserts the reached body's drops and verifies it. The build refuses a package whose
  units, read back and lowered alone, give any function other than the
  library's whole-program lowering.
- A link module record is opaque to the container: it is the linker's
  (`module::package::PackageModule`), `postcard` bytecode of the dotted
  names of the loads its link asked for and its export and namespace
  tables; its row in the module table names the module, and it holds no
  declaration. It decodes when the importer's link reaches the module; one
  that does not decode, or whose row names another source file, links from
  source.
- The importer refuses a package whose `compiler` or `root` differs
  from its own build identity, which covers the library's source, and
  the library root it links against.

## Serialization Inventory

| In-memory data | Treatment |
|---|---|
| `MirDeclarations`, `MirFunction`, blocks, instructions, terms | serialized |
| declaration defaults, conventions, effects, generic declarations | serialized |
| register/slot types and parameter ownership masks | serialized |
| places, loans, interiors, captures, checked call contracts | serialized |
| `Ty`, `TyArg`, `ParamDecl`, constraints, compile-time values | serialized |
| origins, reference signatures, callable environments | serialized |
| source module/path, byte span, optional origin slot | normalized and serialized |
| `SyntaxId` and source AST | deliberately omitted |
| `MirProgram::invariant_errors` | deliberately omitted and recomputed |
| hash-map/set iteration order | derived by canonical sorting |

An assembled artifact is not executable merely because it parses. The
consumer gate is `mir::text::load_artifact`: parse plus the canonical MIR
semantic verifier. Ownership analysis and drop elaboration are producer
obligations the schema cannot re-check — canonical artifacts serialize only
analyzed, drop-elaborated programs. Execution elaborates the loaded program
to concrete MIR and analyzes nothing again.

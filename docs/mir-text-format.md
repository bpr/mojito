# Mojito Textual MIR Format, Version 1.30

This document is the normative specification of Mojito's textual verified-MIR
artifact. The Rust data model in `src/mir.rs` and `src/mir/ir.rs` remains the
in-memory authority; this format is its stable inspection and interchange
boundary. `docs/vm-instruction-set.md` explains execution semantics, while this
document defines syntax and serialized data.

Version 1.30 is implemented end to end for inspection and loading: canonical
in-memory disassembly, full-grammar assembly parsing, and artifact-loading
verification (`mir::text::load_artifact`, which reports canonical-verifier
findings at artifact source spans). Lossless print → parse → print round trips
are enforced byte-for-byte over the drop-elaborated MIR of every executable
corpus fixture by the `roundtrip::*` group of `tests/corpus_test.rs`, and
`mojito exec [FILE]` loads a verified artifact, elaborates it to concrete MIR
from `main` and module initialization, and runs it on the register VM
(`artifact::run_artifact`). The artifact itself stays drop-elaborated MIR,
which may be generic; `exec --erased` runs it as serialized.

## Compatibility

Every artifact begins with exactly:

```text
mojito-mir 1.30
```

The writer emits 1.30. The reader accepts 1.0 through 1.30; *Schema 1.0*
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
`value` register the erased VM reads and whose `expr` is the index the
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
`value` typed the operand's own type, as upstream's `kgen.rebind`. A rebound
place is spelled by its terminal `type`, `Dest`, beside projections typed at
the storage's own. The elaborator asserts the two types equal per instance,
after deciding its `comptime_branch`es, and erases both, so elaborated MIR
holds none and a mismatch fails the instance. A 1.22 consumer rejects the
instruction as unknown.

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
  conformances: [],
  associated_types: []
}
```

`explicit_destructors` contains `destructor { name: symbol, raises: bool }`
records sorted by name. `associated_types` contains
`associated_type { name: symbol, type: type }` records sorted by name.

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

Abstract erased-dispatch requirements have no concrete declaration record.
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
`checked_string`, or `checked_none` with the same fidelity rules.

`CtValue` uses `ct_int`, `ct_uint`, `ct_float_bits`, `ct_int_literal`,
`ct_float_literal`, `ct_bool`, `ct_string`, `ct_tuple`, `ct_list`, `ct_dict`,
`ct_set`, `ct_dtype`, `ct_simd`, `ct_struct { name, fields }`, `ct_type`,
`ct_reflected`, `ct_expr(param-expr)` for a residual parameter expression,
`ct_deferred(binder)` for a slot whose value arrives later (a callable-value
parameter the VM reifies) and which is no part of generic identity, or
`ct_marker(marker)` for an elaborator classification of a name that is no
parameter: `marker_local`, `marker_type`,
`marker_tuple_origin { id, mutability }`, or `marker_applied(int)`, the
folded value of a module constant whose initializer applies a function. A deferred slot names the binder
whose slot it fills by a `binder { owner, slot, name }` record.

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
param_pack_query { pack, query }
param_apply     { function: "symbol", type: meta, args: [param-expr...], evaluated: option<ct-value> }
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
`meta_param_list(meta_type)`. `param_field` is a field of a struct-typed parameter value
that is still a parameter (schema 1.27), `param_reflect` a reflection query over a symbolic subject
(`is_struct()`, `field_count()`, `field_names()`, `field_types()`,
`field_index["name"]()`, `field["name"].T`), and `param_apply` a compile-time
application of a callable symbol, never folded, whose `evaluated` holds the
value the compile-time route established for it when one has (schema 1.14;
`docs/notes/param-expr-attributes.md` §Register types).

`op` is one of `add`, `mul`, `neg`, `sub`, `div`, `floordiv`, `mod`, `pow`,
`shl`, `shr`, `and`, `or`, `xor`, `eq`, `lt`, `le`, `bool_and`, `bool_or`,
`bool_xor`, `cond`. `meta` is `meta_value(type)`, `meta_type`,
`meta_reflected`, `meta_tuple([meta...])`, `meta_list([meta...])`,
`meta_set([meta...])`, `meta_dict([meta_entry { key, value }...])`, or
`meta_param_list(meta)`. A pack query's `pack` is the
`binder { owner, slot, name }` record of the pack binder it queries, and its
`query` is `pack_length`,
`pack_conforms(symbol)`, `pack_predicate { predicate, all }`, or
`pack_contains(param-expr)`.

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
comptime_list(type) tuple([type...]) runtime_pack([type...])
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

`ParamDecl` is
`type_param { owner, slot, name, bounds, callable_bound, default, infer_only, variadic, constraints }`
or
`value_param { owner, slot, name, type, default, callable_default, infer_only, variadic, constraints }`.
Callable defaults use `default_symbol`, `default_parameter`, or
`default_if { condition, then_value, else_value }`.

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
  shared: false
}
```

Projection operations are `field(symbol)`, `index(%rN)`, `const_index(uint)`,
`variant(uint)`, and `uninit_payload`. Each projection pairs with its resulting
type, preserving `projection_tys`. Root and terminal types retain their explicit
optionality for compatibility MIR, although verified production artifacts
require them.

`MirPlace::through`, loan mutability, the shared flag of a `Pointer(to=place)`
alias loan, interior roots/paths, destination domains, invalidation exceptions,
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
| `InvalidateInteriors` | `interiors.invalidate` |
| `MakeRef` / `ReadRef` / `WriteRef` | `ref.make` / `ref.read` / `ref.write` |
| `CopyValue` / `Rebind` | `value.copy` / `value.rebind` |
| `MakeClosure` / `KeepAlive` | `closure.make` / `lifetime.keep_alive` |
| `Const` / `MaterializeLiteral` / `SizeOf` / `TypeName` | `const` / `literal.materialize` / `layout.size_of` / `type.name` |
| `UseVar` / `DefVar` | `var.use` / `var.store` |
| `MovePlace` / `LoadPlace` | `place.move` / `place.load` |
| `UnOp` / `BinOp` | `unary` / `binary` |
| `Call` / `CallIndirect` / `MethodCall` | `call` / `call.indirect` / `call.method` |
| `PointerStorageTake` / `PointerStorageDestroy` | `pointer.take` / `pointer.destroy` |
| `UninitStorage` / `UninitStorageTake` / `UninitStorageDestroy` | `uninit.make` / `uninit.take` / `uninit.destroy` |
| `MarkInitialized` | `ownership.mark_initialized` (schema 1.28: `{ place }`, upstream's `lit.ownership.mark_initialized`) |
| `MarkDestroyed` | `ownership.mark_destroyed` (schema 1.30: `{ place }`, upstream's `lit.ownership.mark_destroyed`: the place holds no value from here and no destructor runs on it) |
| `GetField` | `field.get` |
| `Index` / `Slice` / `MultiIndex` / `MultiSet` | `index.get` / `slice.get` / `index.multi` / `index.multi_set` |
| `Store` / `StoreRef` | `place.store` / `place.store_ref` |
| `MakeTuple` | `tuple.make` |
| `MakeVariant` / `VariantIs` / `VariantGet` / `VariantSet` | `variant.make` / `variant.is` / `variant.get` / `variant.set` |
| `VariantTake` / `VariantSetInitWith` / `VariantDeinitWith` / `VariantReplace` | `variant.take` / `variant.set_init_with` / `variant.deinit_with` / `variant.replace` |
| `MakeSimd` / `SimdCast` / `SimdBitcast` / `SimdShuffle` | `simd.make` / `simd.cast` / `simd.bits` / `simd.shuffle` — `dtype` is a dtype name or `ct_expr(...)`, `width` a lane count or `ct_expr(...)` (schema 1.18); a shuffle's `mask` is a list of lane indices, or a template's `lane_shuffle { lanes: [<param_expr>, ...] }`, `lane_slice { offset: <param_expr>, width: <width> }`, or `lane_join {}`, which elaboration closes (schema 1.26) |
| `Raise` / `Try` | `raise` / `try` |
| `Drop` / `DropVar` | `drop.reg` / `drop.var` |
| `ConsumeVar` / `ConsumePlace` | `consume.var` / `consume.place` |
| `Unsupported` | `unsupported { message: string }` |
| `GetIter` / `HasNext` / `Next` / `TryNext` | `iter.init` / `iter.has_next` / `iter.next` / `iter.try_next` |

`UseMode` is `copy`, `move`, `borrow_shared`, or `borrow_mut`. Intrinsic
subscripts are `tuple_storage`, `variadic_storage`, `simd`, `pointer`, and
`comptime_list`. Slice descriptors are `slice`, `contiguous_slice`, and
`strided_slice`. Result adapters currently contain only
`copy_iterator_reference`.

For `Try`, `body`, `handler`, `orelse`, and `finalbody` contain lists of local
blocks. Handler absence differs from a present empty handler. All call fields,
including caller places, capture accesses, compile-time arguments, instantiated
contracts, reference-result ABI, and checked subscript contracts, are required
as explicit options/lists. Backends must not reconstruct omitted selections.

## Terminators

| MIR variant | Canonical record |
|---|---|
| `Jump(target)` | `jump { target: bbN }` |
| `Branch` | `branch { condition: %rN, then: bbN, else: bbN }` |
| `ComptimeBranch` | `comptime_branch { condition: <constraint>, then: bbN, else: bbN }` — a `comptime if` on a parameter expression over the function's binders (the `availability` constraint grammar), decided by the elaborator; concrete MIR carries none |
| `ComptimeFor` | `comptime_for { binder: <binder>, slot: $vN, start: <param_expr>, stop: <param_expr>, step: <param_expr>, body: bbN, exit: bbN }` for a `range`, `comptime_for.elements { binder: <binder>, slot: $vN, elements: <param_expr>, body: bbN, exit: bbN }` for any other sequence — a `comptime for` header: a loop whose variable is the binder `binder`, read by the body through the slot `slot`, over the integers the three parameter expressions span as `range` does, or over the elements the `elements` value yields (a list's or a set's elements, a dictionary's keys); the body's back edge jumps to the header, the elaborator unrolls it, and concrete MIR carries none (schema 1.16; the `elements` form and the field name `binder`, `index` before, schema 1.25) |
| `Return` | `return { value: option<reg> }` |
| `ReturnWithCleanup` | `return.cleanup { value: option<reg>, cleanup: [var...] }` |
| `FallOff` | `falloff {}` |
| `EscapeJump` | `escape { target: bbN, cleanup: [var...] }` |

Unknown terminators and instructions are fatal for schema major version 1.

## Complete Artifact Example

```text
mojito-mir 1.30
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

# Mojito Textual MIR Format, Version 1.0

This document is the normative specification of Mojito's textual verified-MIR
artifact. The Rust data model in `src/mir.rs` and `src/mir/ir.rs` remains the
in-memory authority; this format is its stable inspection and interchange
boundary. `docs/vm-instruction-set.md` explains execution semantics, while this
document defines syntax and serialized data.

Version 1.0 is a schema. Canonical in-memory disassembly, seed-subset assembly
parsing, and artifact-loading verification (`mir::text::load_artifact`, which
reports canonical-verifier findings at artifact source spans) are implemented;
full-coverage round trips and CLI execution remain separate implementation
stages.

## Compatibility

Every artifact begins with exactly:

```text
mojito-mir 1.0
```

The two unsigned decimal components are major and minor versions. A consumer
must reject an unknown major version. Within major version 1, a newer minor may
add fields only to an `optional { ... }` record or add named capabilities to the
header. Unknown optional fields and capabilities may be skipped; unknown
required fields, types, instructions, terminators, or enum tags are errors. A
semantic or executable addition therefore requires consumer support and cannot
be hidden in optional metadata. Removing or changing existing syntax requires a
new major version.

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
  explicit_destructors: []
}
```

`explicit_destructors` contains `destructor { name: symbol, raises: bool }`
records sorted by name.

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
  ref_params: [false, false]
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
`int_literal(decimal)`, `float_literal(exact_decimal)`, `bool(true|false)`,
`string(string)`, `function(symbol)`, or `none`. Concrete `float` stores the
exact IEEE-754 binary64 bits as 16 lowercase hex digits. `IntLiteral` uses
arbitrary-precision decimal. `FloatLiteral` uses its canonical exact decimal
spelling, including negative zero, and may not round through host `f64`.

`CheckedConst` uses `checked_int`, `checked_float`, `checked_bool`,
`checked_string`, or `checked_none` with the same fidelity rules.

`CtValue` uses `ct_int`, `ct_uint`, `ct_float_bits`, `ct_int_literal`,
`ct_float_literal`, `ct_bool`, `ct_string`, `ct_tuple`, `ct_list`, `ct_dtype`,
`ct_struct { name, fields }`, `ct_type`, `ct_reflected`, or `ct_param`.

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
param { name, bounds, callable_bound }
assoc { base, member }
dependent_index { elements, index }
struct_type { name, arguments }
simd { dtype, width }
comptime_list(type) tuple([type...]) runtime_pack([type...])
variadic_pack(type) variant([type...])
pointer { element, origin }
ref { referent, origin, mutability }
```

`TyArg` tags are `type_arg`, `value_arg`, and `origin_arg`. DTypes and all AST
operators/conventions use their lowercase source-independent enum names;
conventions are `read`, `var`, `mut`, `out`, `ref`, and `deinit`.

`ParamDecl` is `type_param { name, bounds, default, constraints }` or
`value_param { name, type, default, variadic, infer_only, constraints }`.
Callable defaults use `default_symbol`, `default_parameter`, or
`default_if { condition, then_value, else_value }`.

`GenericConstraint` and `CtExpr` are prefix trees. Their tags map one-to-one to
the public variants: `conforms`, `conforms_pack`, `pack_predicate`,
`pack_contains`, `trivial`, `eq`, `ne`, `lt`, `le`, `gt`, `ge`, `and`, `or`,
`not`, `constraint_bool`; and `ct_param`, `ct_value`, `ct_neg`, `ct_add`,
`ct_sub`, `ct_mul`, `ct_floor_div`, `ct_mod`, `ct_pow`. Constraint operands are
`operand_param`, `operand_value`, `operand_type`, and `operand_pack_length`.

### Origins and callable environments

Origin path segments are `field(symbol)`, `any_index`, `interior(symbol)`, and
`subtree`. Origins are `origin_param(id)`, `origin_self`,
`origin_place { root: $vN, path }`, `origin_union`, `origin_static`, and
`origin_untracked { mutable }`. Pointer origins add `pointer_param`,
`pointer_self`, and `pointer_unsafe_any`; all fields of the corresponding
`PointerOrigin` variant are required.

Mutability is `immutable`, `mutable`, or `mutability_param(id)`. Signature
origins are `sig_self`, `sig_param(index)`, `sig_bound(origin)`, `sig_static`,
`sig_untracked`, `sig_projected`, `sig_union`, and `sig_infer`. Signature
mutability is `sig_immutable`, `sig_mutable`, `sig_bool_param(index)`, or
`sig_infer`. A `RefSig` is `ref_sig { origin, mutability }`.

Capture access is `read`, `write`, or `infer`. Capture sets are
`capture_set_param(id)` or `capture_set([capture_origin...])`. Callable
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
  })
}
```

Projection operations are `field(symbol)`, `index(%rN)`, `const_index(uint)`,
`variant(uint)`, and `uninit_payload`. Each projection pairs with its resulting
type, preserving `projection_tys`. Root and terminal types retain their explicit
optionality for compatibility MIR, although verified production artifacts
require them.

`MirPlace::through`, loan mutability, interior roots/paths, destination domains,
invalidation exceptions, and capture accesses are semantic data even when the
VM erases them. Verification must prove that a through slot is a compatible
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
| `CopyValue` | `value.copy` |
| `MakeClosure` / `KeepAlive` | `closure.make` / `lifetime.keep_alive` |
| `Const` / `MaterializeLiteral` | `const` / `literal.materialize` |
| `UseVar` / `DefVar` | `var.use` / `var.store` |
| `MovePlace` / `LoadPlace` | `place.move` / `place.load` |
| `UnOp` / `BinOp` | `unary` / `binary` |
| `Call` / `CallIndirect` / `MethodCall` | `call` / `call.indirect` / `call.method` |
| `PointerStorageTake` / `PointerStorageDestroy` | `pointer.take` / `pointer.destroy` |
| `UninitStorage` / `UninitStorageTake` / `UninitStorageDestroy` | `uninit.make` / `uninit.take` / `uninit.destroy` |
| `GetField` | `field.get` |
| `Index` / `Slice` / `MultiIndex` / `MultiSet` | `index.get` / `slice.get` / `index.multi` / `index.multi_set` |
| `Store` / `StoreRef` | `place.store` / `place.store_ref` |
| `MakeTuple` | `tuple.make` |
| `MakeVariant` / `VariantIs` / `VariantGet` / `VariantSet` | `variant.make` / `variant.is` / `variant.get` / `variant.set` |
| `VariantTake` / `VariantSetInitWith` / `VariantDeinitWith` / `VariantReplace` | `variant.take` / `variant.set_init_with` / `variant.deinit_with` / `variant.replace` |
| `MakeSimd` / `SimdCast` / `SimdShuffle` | `simd.make` / `simd.cast` / `simd.shuffle` |
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
| `Return` | `return { value: option<reg> }` |
| `ReturnWithCleanup` | `return.cleanup { value: option<reg>, cleanup: [var...] }` |
| `FallOff` | `falloff {}` |
| `EscapeJump` | `escape { target: bbN, cleanup: [var...] }` |

Unknown terminators and instructions are fatal for schema major version 1.

## Complete Artifact Example

```text
mojito-mir 1.0
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
    ref_params: [false]
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

An assembled artifact is not executable merely because it parses. It must pass
the canonical MIR semantic verifier, ownership analysis, and drop-elaboration
contract before a backend accepts it.

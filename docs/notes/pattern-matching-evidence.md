# Pattern matching: pinned evidence

Evidence for the pattern-matching entries (R1132–R1143), gathered 2026-10-17 against
`Mojo 1.2.0.dev2026101705 (46f49943)` (upstream checkout `cfd79ca736`).
Every row names the probe that pins it; runnable probes live under
`conformance/probes/match_*.mojo`, `destructure_*.mojo`, `if_case_*.mojo`,
and `comptime_match_forms.mojo`, each with the pin's output in its header.
Rows marked *diagnostic only* were run once and are recorded here rather
than kept as files, with the pin's exact text. A probe Mojito runs with the
pin's output moves to `assets/ok/` (or `assets/type_error/`) under the same
name; the probes still under `conformance/probes/` say which roadmap entry
they wait for.

## Evaluation and effects

| Question | Pin behavior | Probe |
|---|---|---|
| Subject evaluation | Once per `match`; a failed arm leaves it for the next arm. | `match_guard_copy_drop`, `match_temporary_subject_survives_failed_var_case` |
| `== expr` ordering | The expression is evaluated and `__eq__` called once per test, in case order; a failed tuple element short-circuits the rest of that case. | `match_equality_effects` |
| Raising `__eq__` | An ordinary call: `cannot call function that may raise in a context that cannot raise`; inside `try` the error is caught, it is not a mismatch. | diagnostic only (`raising_eq`) |
| Sequence accessors | One length test per case, upstream's `std.builtin._stubs.list_length_matches(subject, arity, exact)` calling `__len__`, gates every element read; a wrong length reads nothing. Each element is `subject[i]` through ordinary subscript resolution, read once per case at the first command that touches it, in pattern order (`[a, 11, c]` prints `get 0, get 1, get 2`; `[a, 99, c]` stops after `get 1`); `_` reads nothing; each `\|` alternative re-reads (`[1 \| 2 \| 3, _, var c]` over base 3 reads `get 0` three times); a from-end element calls `__len__` again. An owned getter result is an rvalue: a bare binding keeps it alive to its last use, `var c` moves it (no copy), a test-only element dies after its test, a failing case destroys its reads in reverse order; `as` and `ref` over a register-passable one reject (`value of type 'Int' doesn't have a memory origin in 'ref' binding`), `ref p` over a `String` one is accepted. A raising getter is an ordinary raising call. | `match_sequence_custom_sized`, `assets/ok/match_sequence_fixed_length.mojo` |
| Starred remainder | Requires `__getitem__` over a slice: `invalid call to '__getitem__': value passed to 'i' cannot be converted from slice literal to 'Int'` on a type with only the `Int` overload. A pattern star needs it only when its sub-pattern reads (`*_` does not); a target star always does, `*_` included. | `assets/type_error/match_sequence_star_needs_slice_getitem.mojo`, `destructure_starred_discard_slices.mojo` |
| Guard | Runs after the case's bindings exist; `False` advances to the next case, not another OR alternative. | `match_guard_copy_drop`, `match_or_alternatives_bind` |
| Exits | `continue`/`break` in a case target the enclosing loop; `return` leaves the function. | `match_exits_and_return_analysis` |

## Bindings

| Question | Pin behavior | Probe |
|---|---|---|
| Bare binding | Immutable; over a memory value it is an immutable reference (no copy, even for a non-copyable subject). Writing it: `expression must be mutable in assignment`. | `match_noncopyable_subject`, `match_binding_kinds_on_temporary`, diagnostic only (`bindings_scope2`) |
| `var` binding | Copies with the implicit-copy rule (`value of type 'Tracked' cannot be implicitly copied, it does not conform to 'ImplicitlyCopyable'`); the copy is destroyed after its last use, which may be before the guard call. A last-use copy of a dead subject is elided to a move. | `match_guard_copy_drop`, `match_temporary_subject_survives_failed_var_case` |
| `ref` binding | Writes through to a mutable place; over a temporary it is immutable (`invalid use of mutating method on rvalue`). `match Int(): case ref z` rejects: `value of type 'Int' doesn't have a memory origin in 'ref' binding`. | `match_binding_kinds_on_temporary`, `match_optional_enum_cases`, upstream `match_diags.mojo` |
| `as` binding | Keeps the subject's category: writes through to a mutable place (plan probe `bindings.mojo`), immutable over a temporary. | diagnostic only (`as_temporary`: `expression must be mutable in assignment`) |
| Scope | A case's names exist in its body only and may shadow outer names; `if case` names are absent from `else`. | `match_binding_scope`, diagnostic only (`if_case_scope`: `use of unknown declaration 'k'`) |
| Duplicate name | `invalid redefinition of 'a'` with a `previous definition here` note. | diagnostic only (`bind_dup`) |
| OR joins | Alternatives must bind the same names (`or-pattern alternatives must bind the same names; 'a' is bound in one alternative but not the other`), with one kind (`or-pattern binding 'x' must use the same 'var'/'ref' kind in each alternative`) and one type (`or-pattern binding 'n' has incompatible types across alternatives`, note `first alternative has type 'Tracked', this alternative has type 'Int'`). | diagnostic only (`temp_consumed` first draft) |
| Nested `var ref x` | A warning upstream (`nested 'var' or 'ref' patterns are redundant, remove the outer pattern`); Mojito may reject it. | upstream `match_diags.mojo` |

## Patterns and shapes

| Question | Pin behavior | Probe |
|---|---|---|
| Literal forms | Int, float, string, `True`/`False`, `None` compare through `__eq__`; a signed literal is not a pattern (`expression is not a valid match pattern; use '==' to compare against its value`, fix-it `==`). A type-mismatched literal is the `__eq__` call's error. | `match_literal_forms`, diagnostic only (`coverage_texts`) |
| `case Int:` | Binds a name `Int`. | `match_literal_forms` |
| `==` operand | Parsed at unary (factor) precedence: `== a \| b` is `(== a) \| b`; `== compute()` and `== {1, offset=2}` are fine. | upstream `ParserExprs.cpp`, `match.mojo` |
| `as` precedence | Looser than `\|`: `3 \| 4 as n` is `(3 \| 4) as n`; it binds per comma element. | `match_or_alternatives_bind` |
| Comma subjects/patterns | `match a, b:` and `case True, _:` form tuples without parentheses; `()` and `(n,)` work. | `match_literal_forms` |
| Tuple arity | Static: `cannot match value of 'Tuple[Int, Int]' of 2 elements against a pattern with 3 elements`; non-tuple subject: `expected a tuple type to match against, got 'Int'`. | diagnostic only (`tuple_shape_errors`) |
| Struct patterns | `Point(x=0, y=yv)`, contextual `{x=1}`, nested, omitted fields are wildcards, `ref` field bindings. Positional arguments reject (`struct patterns do not support positional or unpacked arguments`); unknown field `'w' is not a field of 'Vec3'`; wrong head `cannot match value of type 'Vec3' against pattern type 'Int'`; duplicate field `keyword argument 'x' was already used; remove the duplicate`; empty `String()` warns. | `match_struct_and_initializer_patterns`, upstream `match_diags.mojo` |
| Enum cases | `.Some(x)`, `.None`, `Optional.Some(0)`, `.Some(ref s)`; one payload subpattern (`enum case patterns currently support at most one payload subpattern`); `.None()` and `.None(x)`: `enum case 'None' has no associated value`; `.Some()`: `enum case 'Some' requires a payload pattern inside the parentheses`; unknown `'Nope' is not a case of 'Optional[Int]'`; a non-enum subject: `cannot resolve inferred member without a contextual type`; a parametric enum: `cannot match on a parametric enum type`. Observed while landing R1133 (2026-10-20): the owner of a qualified case is checked only at the root and nominally (`Optional[String].Some(q)` over an `Optional[Int]` binds the `Int`; `(Rec.Some(z), _)` runs unchecked); a temporary subject's payload is read immutably (`.Some(ref s)` then `s += "!"` is rejected) while a field projection and a root `ref` stay mutable; consecutive cases sharing a leading tag test read the discriminant once per cluster, a `\|` reading it per alternative; a qualified or leading-dot head over a subject that is no `EnumLike` is an equality test; and a custom conformer with a repeated case name crashes the pin at run time when a `case _` follows its cases. | `match_optional_enum_cases`, diagnostic only (`optional_errors`), upstream `match_diags.mojo`; Mojito `assets/ok/match_enum_cases_*.mojo` |
| Sequence patterns | `[]`, `[x]`, `[1, *mid, 5]`, `[*_, 5]`, `[*all]` over `List`, a user `Sized` + `__getitem__(Int)` type, a generic `T: Seq` whose trait refines `Sized` and requires `__getitem__`, a temporary, a `Tuple`, and inside a tuple pattern; a wrong length advances to the next case. The subject's static type must conform to `Sized` by declaration: `value of type 'String' does not conform to 'Sized'`, the same for a struct with `__len__` that does not declare it, `cannot prove that value of type 'T' conforms to 'Sized'` for a type parameter; a `Sized` subject without a getter: `` 'S' is not subscriptable, it does not implement the `__getitem__`/`__setitem__` methods ``. | `assets/ok/destructure_starred_slice.mojo`, `match_sequence_custom_sized.mojo`, `match_sequence_fixed_length.mojo`, `assets/type_error/match_sequence_*` |
| Sequence element patterns | A list element path is untyped, so only `_`, bindings, literals, `==`, `as`, and `\|` of those work inside one: `[(u, v)]` gives `expected a tuple type to match against, got <<NULL TYPE>>`, `[P(x=q)]` `cannot match value of type <<NULL TYPE>> against pattern type 'P'`, `[[u]]` `value of type <<NULL TYPE>> does not conform to 'Sized'`. The pin crashes without a diagnostic on `[{x=q}]` and on an `Int`, `Float64`, or `SIMD` subject; Mojito rejects each. | `assets/type_error/match_sequence_element_*` |
| Sequence coverage | No coverage warnings: a repeated `[_, _]` is not unreachable, and a match of only sequence cases is not "not exhaustive". `[1 \| 3, == 2]` warns `'==' is redundant when matching a literal; remove it`. | diagnostic only |
| Remainder type | `Span[Int, origin_of(values["element"])]` for a `List[Int]` (`cannot implicitly convert 'Span[...]' value to 'List[Int]'`). The star takes any closed pattern: `*var vs` copies the span; `*ref rr` is `value of type 'Span[...]' doesn't have a memory origin in 'ref' binding`, `Span` being register-passable; `*rr as whole` is a parse error. The slice path is untyped: `*[a, b]` and `*(a, b)` reject as element paths do. | `assets/ok/sequence_starred_forms.mojo`, `assets/type_error/match_sequence_star_*` |
| Starred pattern order | `[u, *mm, v, w]` over a printing 5-element `Seq` prints `len, get 0, len, slice 1 3, len, get 3, len, get 4`: every from-end element and slice end reads `__len__` afresh; `[*_, v, w]` prints `len, len, get 3, len, get 4`. A remainder of a temporary subject keeps it alive until the remainder's last use. | `assets/ok/sequence_starred_evaluation_order.mojo`, `sequence_starred_temporary_lifetime.mojo` |
| Case layouts | Same-indent and indented `case` blocks, same-line bodies, nested `match`. No case: `'match' statement must have at least one 'case' block`. | `match_case_layouts`, upstream `match_diags.mojo` |

## Coverage and diagnostics

| Question | Pin behavior | Probe |
|---|---|---|
| Non-exhaustive `Bool` | Warning `'match' is not exhaustive; missing case for 'False'`; a product lists cells: `missing cases for 'False, True' and 'True, False'`. The program still runs. | diagnostic only (`coverage_texts`) |
| Duplicate literal | Warning `case is unreachable; '0' is already covered by a previous case`. | diagnostic only (`coverage_texts`) |
| After a catch-all | Warning `case is unreachable; previous cases cover every value of the match subject`. | diagnostic only (`coverage_texts`, `optional_errors`) |
| `Optional` without `.None` | Warning `'match' is not exhaustive; missing case for 'None'`. | diagnostic only (`optional_errors`) |
| Return analysis | An exhaustive `Bool` match or a `_` case whose arms all return satisfies it; a guarded last case does not (`return expected at end of function with results`). | `match_exits_and_return_analysis`, diagnostic only (`exits`) |
| Irrefutable `if case` | Accepted silently; the `else` of an irrefutable tuple condition warns `unreachable code after match that always completes`. | `if_case_and_while_case` |
| Constant subjects | A literal subject folds the match; the pin then warns about unreachable code after it. Not a semantic requirement. | `match_literal_forms` |

## Destructuring assignment

| Question | Pin behavior | Probe |
|---|---|---|
| Order | Destination subexpressions (indices) evaluate left to right *before* the right-hand side; the RHS evaluates once; stores follow. `(i, xs[i]) = (2, 7)` writes `xs[0]`. | `destructure_evaluation_order` |
| Temporaries | Elements copy out of a temporary tuple (implicit-copy rule applies; `_` copies nothing); the temporary is then destroyed. `tpair()^` changes nothing (`transfer from an owned value has no effect`). | `destructure_evaluation_order` |
| Nested targets | Places, `var`, `ref`, `_`, and tuples nest freely: `(a, (b, c))`, `var (x, (y, z))`, `var (p, q), r = (7, 8), 9`, `(xs[0], (xs[1], a))`, `(var m, ref n)`, `_, (b, _)`. | `destructure_nested_targets` |
| Invalid targets | `(var a, 0) = p`: `expression must be mutable in assignment`; `var (x, y, z) = p` on a pair: `cannot declare 'x' without a contextual type from its initializer`; `w, = p`: `cannot implicitly convert 'Tuple[Int, Int]' value to 'Tuple[Int]'`. | diagnostic only (`tuple_shape_errors`) |
| Lists | `var [a, b] = values` requires a raising context (`cannot call function that may raise in a context that cannot raise`); the value is evaluated once, then `check_list_length` raises `list pattern does not match value length` before any store and before any element read; `[x, y] = values` leaves `x`, `y` unchanged. Then, per element in order: `value.__getitem__(i)` (an attribute call, not a subscript), that destination's own subexpressions, the store — `[xs[idx(0)], xs[idx(1)]] = make()` prints `make, len, get 0, idx 0, get 1, idx 1`. `_` reads its element (an owned result dies at once); owned getter results move into `var` leaves; nested tuple and list targets run; a `ref` leaf is immutable. A starred `[a, *m, b, c]` prints `len, len, get 0, slice 1 3, get 3, get 4`: `check_list_length(value, 3, False)`, one more `__len__` for the suffix start, then the prefix, the slice subscript (`[1:]` with no end when the star is last), and the suffix at `start + k`; `*_` slices too and discards; the star takes any target (`*var m`, a place, `*[g, *h]`); `*(a, b)` is `cannot unpack value of type 'Span[...]' into 2 values`; mutating or moving the source before the remainder's last use is rejected. | `destructure_sequence_targets`, `destructure_list_wrong_length_raises`, `destructure_starred_slice`, `sequence_starred_*`, `destructure_starred_*` in the error folders |
| List-target rejections | `String` value: `value of type 'String' does not conform to 'Sized'`; a struct with `__len__`/`__getitem__` not declaring `Sized`: `value of type 'S' does not conform to 'Sized'`; a `Sized` struct without a getter, or a `Tuple` value: `'S' value has no attribute '__getitem__'`; `[p, q] = values` with undeclared names: `implicit declaration of 'p' is not allowed`; `[1, b] = values`, and a subscript leaf whose getter returns by value (even beside `__setitem__`): `expression must be mutable in collection literal`. `Int` and `SIMD` values run (`__len__` is 1 or the width). `var ([a, b], c) = (values.copy(), 3)` runs. The aliasing swap `[xs[1], xs[0]] = xs` runs and prints `1 1`. | `destructure_sequence_*` in the error folders |
| Nested convention under a declaring prefix | `var [c, var d] = values` and `var (ref a, b) = pair` declare nothing for the nested leaf: a later use of `d` or `a` is `use of unknown declaration`. | diagnostic only (R1225) |
| Iteration targets | `for a, b in pairs`, `for (a, b)`, `for (var a, _)`, `for x, (y, z)`, and `[a * b for a, b in pairs]` work; `for ref (a, b)` leaves `a` immutable (`expression must be mutable for in-place operator destination`). | diagnostic only (`for_targets`) |

## Conditional and compile-time forms

| Question | Pin behavior | Probe |
|---|---|---|
| `if case` / `elif case` | Each RHS is evaluated only when its branch is reached; mixes with boolean branches. A trailing `if` guard is not accepted (parsed as a ternary: `expected 'else' clause in ternary`). | `if_case_and_while_case`, `if_case_mixed_with_boolean_elif` |
| `while case` | Re-evaluates the subject each iteration; `else` runs on failure, `break` skips it. | `if_case_and_while_case` |
| `comptime match` | `== Int` over a type parameter, literals and tuples over values, bare bindings, OR, `as`, guards. `var` bindings reject (`'var' bindings are not supported in 'comptime match'`); a dynamic subject: `cannot use a dynamic value in 'match' subject`; a dynamic guard: `cannot use a dynamic value in 'comptime if' condition`. | `comptime_match_forms`, diagnostic only |
| Untaken arm bodies | Still checked: `var bad: Int = "str"` in an untaken arm is `cannot implicitly convert 'StringLiteral["str"]' value to 'Int'`. | diagnostic only (`comptime_match` first draft) |

## EnumLike prerequisites

`EnumLike` (`stdlib/std/builtin/enum_like.mojo`) needs, beyond what Mojito
has today:

- `_MLIR.KGENParamListType[T]` as the type of a raw parameter list
  (`comptime _enum_case_names: _MLIR.KGENParamListType[KGENString]`), the
  spelling R807 ports.
- `TypeList[Trait=AnyType, Self._enum_case_types]()[id]` as a dependent
  result type, which needs R1168's library `TypeList` and its
  `__getitem_param__` read of a type pack.
- `ParameterList.of[...]` / `TypeList.of[...]` `.values` projections, also
  R807.

`Optional` and `Bool` conform upstream through those spellings; Mojito's
`Optional` keeps its payload storage and gains the three members once the
spellings exist (R1132).

## Settled uncertainties from the plan

- *Copy/destructor ordering on failed guards and OR joins*: copies precede
  the guard and die at last use; a failed OR alternative's bindings do not
  outlive the alternative (tests precede binds; `match_or_alternatives_bind`).
- *Assignment ordering*: destinations before RHS, stores after; see above.
- *Wrong-length assignment*: raises `list pattern does not match value
  length` with nothing stored; a `raises` context is mandatory.
- *Star-discard accessor behavior*: in a pattern `*_` performs no slice; a
  target `*_` slices and discards; a named star slices through
  `__getitem__(Slice)` and keeps the result's origin.
- *Generic EnumLike*: rejected by the pin (`cannot match on a parametric
  enum type`), so Mojito's subset boundary matches.
- *Compile-time conditional-pattern statements*: `comptime if case` exists
  upstream (`ParserStmts.cpp` diagnoses `variable binding ... in 'comptime
  if case' must be a compile-time value`); it is not scheduled here and is
  filed separately when M18 lands.

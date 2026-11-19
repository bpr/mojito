# Upstream's `!lit.origin<mut>` and `!lit.ref<T, origin._mlir_origin[, 0]>`
# spellings: `_lit_origin_type_of_mut[mut]` (std.origin's private alias, and
# a program's own copy) types a value parameter over a fixed or a symbolic
# mutability, and a struct's `comptime` names the `!lit.ref` to `T` under its
# origin parameter, read through a function binder or a place's origin.
from std.origin import _lit_origin_type_of_mut

comptime _L[mut: Bool] = __mlir_type[`!lit.origin<`, mut._mlir_value, `>`]


struct Fixed[o: _L[True]]:
    pass


struct Symbolic[m: Bool, o: _lit_origin_type_of_mut[m], //]:
    pass


struct View[T: AnyType, o: ImmOrigin]:
    comptime Ref = __mlir_type[
        `!lit.ref<`, Self.T, `, `, Self.o._mlir_origin, `, `, `0`, `>`
    ]
    comptime GenericRef = __mlir_type[
        `!lit.ref<`, Self.T, `, `, Self.o._mlir_origin, `>`
    ]


def read[o: ImmOrigin](p: Pointer[Int, o]) -> Int:
    comptime R = View[Int, o].Ref
    return p[]


def main():
    var x = 41
    comptime R = View[Int, origin_of(x)].Ref
    comptime G = View[Int, origin_of(x)].GenericRef
    print(read(Pointer(to=x)) + 1)

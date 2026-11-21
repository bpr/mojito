# `Pointer`'s casts, `_get_kgen_pointer`, `copy`, and its `_mlir_value=` and
# `copy=` constructions are the declared struct's bodies
# (`std/memory/pointer.mojo`): a `Bool` parameter spells `unsafe_mut_cast`,
# `as_imm` passes as an immutable pointer, an origin cast to a place keeps
# that place alive, and a `!kgen.pointer` rebuilds a pointer over a named
# origin.
from std.memory import Pointer


def take_imm(p: ImmPointer[Int, _]) -> Int:
    return p[]


def flip[m: Bool](p: Pointer[Int, _]) -> Int:
    return p.unsafe_mut_cast[m]()[]


def main():
    var x = 5
    var y = 7
    var p = Pointer(to=x)
    print(take_imm(p.as_imm()), flip[False](p), flip[True](p))
    var q = p.unsafe_origin_cast[origin_of(y)]()
    print(q[])
    print(p.unsafe_bitcast[Int]()[], p[])
    print(p.unsafe_address_space_cast()[], p[])
    var c = p.copy()
    var d = Pointer(copy=p)
    c[] += 1
    print(d[], x)
    var k = p._get_kgen_pointer()
    var r = Pointer[Int, origin_of(x)](_mlir_value=k)
    r[] += 1
    print(r[], x, y)

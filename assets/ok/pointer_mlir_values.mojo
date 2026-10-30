# `Pointer`'s raw MLIR values: the `!kgen.pointer` it wraps and the
# `!lit.ref` it is built from.


def read[o: ImmOrigin](r: Pointer[Int, o]._mlir_lit_ref) -> Int:
    return Pointer(_mlir_value=r)[]


def widen(k: __mlir_type[`!kgen.pointer<`, Int, `>`]) -> Int:
    return Pointer[Int, ImmUntrackedOrigin](_mlir_value=k)[]


def main():
    var x = 7
    print(read(__get_mvalue_as_litref(x)))
    var k = Pointer(to=x)._get_kgen_pointer()
    print(widen(k))
    var copy = Pointer(to=x)._mlir_value
    var p = Pointer[Int, origin_of(x)](_mlir_value=copy)
    var q = Pointer[_, origin_of(x)](_mlir_value=k)
    p[] = 9
    print(q[])
    print(x)
    var r = __get_mvalue_as_litref(x)
    Pointer(_mlir_value=r)[] = 11
    print(x)

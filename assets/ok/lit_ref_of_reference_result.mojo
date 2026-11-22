# `__get_mvalue_as_litref` of an mvalue that already is a reference gives
# that reference: over `p: Pointer[Int, Self.o]`, the getter's result
# `p[]` is `ref[Self.o] Int`, so it stores into a `!lit.ref` field typed
# over the struct's own origin, and a `Pointer` rebuilt from the field
# reads the referent.
struct Holder[o: ImmOrigin]:
    var r: Pointer[Int, Self.o]._mlir_lit_ref

    def __init__(out self, p: Pointer[Int, Self.o]):
        self.r = __get_mvalue_as_litref(p[])


def first[o: ImmOrigin](p: Pointer[Int, o]) -> Pointer[Int, o]._mlir_lit_ref:
    return __get_mvalue_as_litref(p[])


def main():
    var x = 7
    var h = Holder(Pointer(to=x))
    print(Pointer(_mlir_value=h.r)[])
    var y = 11
    print(Pointer(_mlir_value=first(Pointer(to=y)))[])

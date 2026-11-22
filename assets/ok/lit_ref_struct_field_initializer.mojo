# A `!lit.ref` initializer parameter over a struct's own origin binder binds
# that binder from its argument, explicitly bracketed or inferred, and the
# stored reference reads back through a field, a result, and a `Pointer`.
struct Holder[o: ImmOrigin]:
    var r: Pointer[Int, Self.o]._mlir_lit_ref

    def __init__(out self, var r: Pointer[Int, Self.o]._mlir_lit_ref):
        self.r = r


def get[o: ImmOrigin](h: Holder[o]) -> Pointer[Int, o]._mlir_lit_ref:
    return h.r


def main():
    var x = 7
    var explicit = Holder[origin_of(x)](__get_mvalue_as_litref(x))
    print(Pointer(_mlir_value=explicit.r)[])
    var y = 9
    var inferred = Holder(__get_mvalue_as_litref(y))
    print(Pointer(_mlir_value=get(inferred))[])

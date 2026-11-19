# `Origin` declares upstream's `_mlir_origin: _lit_origin_type_of_mut[mut]`
# beside its `mut`, so the raw origin `X._mlir_origin` binds it and `mut`
# infers from that origin's mutability, a mutable origin narrowing to an
# explicit `mut=False`; an `Origin` type's `mut`, and a struct's `mut=`
# binder, read back as type members.


struct V[mut: Bool, //, o: Origin[mut=mut]]:
    def __init__(out self):
        pass


def main():
    var x = 1
    var y = 2
    comptime O = Origin[_mlir_origin=origin_of(x)._mlir_origin]
    print(O.mut)
    comptime N = Origin[mut=False, _mlir_origin=origin_of(x)._mlir_origin]
    print(N.mut)
    comptime T = Origin[mut=True]
    print(T.mut)
    print(ImmOrigin.mut)
    print(MutOrigin.mut)
    print(Span[Int, origin_of(y)].mut)
    print(Span[Int, ImmOrigin(origin_of(y))].mut)
    print(V[origin_of(y)].mut)
    x += y
    print(x)

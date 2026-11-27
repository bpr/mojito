# An origin binder typed by an alias of the unapplied `Origin` (`[o: O]`
# over `comptime O = Origin`) declares upstream's implicit `mut` and
# `_mlir_origin` binders, as one spelled `Origin[mut=m, _mlir_origin=r]`
# declares its own: a call binds both from the origin its argument or
# bracket binds, so `o.mut` reads that origin's mutability.
from std.origin import _lit_origin_type_of_mut

comptime O = Origin


def by_ref[o: O](ref[o] x: Int) -> Bool:
    return o.mut


def by_pointer[o: O](p: Pointer[Int, o]) -> Bool:
    return o.mut


def spelled[
    m: Bool, r: _lit_origin_type_of_mut[m], //, o: Origin[mut=m, _mlir_origin=r]
](ref[o] x: Int) -> Bool:
    return m


def main():
    var n = 4
    print(by_ref(n))
    print(by_ref(4))
    print(by_ref[origin_of(n)](n))
    print(by_ref[ImmOrigin(origin_of(n))](n))
    print(by_pointer(Pointer(to=n)))
    var q: Pointer[Int, ImmOrigin(origin_of(n))] = Pointer(to=n)
    print(by_pointer(q))
    print(spelled(n))
    print(spelled(4))

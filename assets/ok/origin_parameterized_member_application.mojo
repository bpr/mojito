# A struct's origin-parameterized `comptime` member applied to an origin
# binds the member's own origin binder: `S.W[origin_of(x)]` is
# `Pointer[Int, origin_of(x)]`, and the bundled `Pointer._with_origin` and
# `_OriginCastType` take the applied origin's mutability, in a type
# annotation and in a `comptime` value alike.


struct S:
    comptime W[o: Origin] = Pointer[Int, o]
    var a: Int

    def __init__(out self):
        self.a = 0


def main():
    var s = S()
    var x = 5
    var y = 6
    var w: type_of(s).W[origin_of(x)] = Pointer(to=x)
    print(w[])
    var p = Pointer(to=x)
    var q: type_of(p)._with_origin[origin_of(y)] = Pointer(to=y)
    print(q[], q.mut)
    comptime Q = type_of(p)._OriginCastType[origin_of(y)]
    var r: Q = Pointer(to=y)
    print(r[], r.mut)
    var i: type_of(p)._with_origin[ImmOrigin(origin_of(y))] = Pointer(to=y)
    print(i[], i.mut)

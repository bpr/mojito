# expect: expected a mutable origin for parameter 'o'
# An `ImmOrigin(o)` argument cannot fill an `Origin[mut=True]` binder.
struct Holder[o: Origin[mut=True]]:
    var p: Pointer[Int, Self.o]

    def __init__(out self, ref [Self.o] x: Int):
        self.p = Pointer(to=x)


def main():
    var x = 7
    var h: Holder[ImmOrigin(origin_of(x))] = Holder(x)
    print(h.p[])

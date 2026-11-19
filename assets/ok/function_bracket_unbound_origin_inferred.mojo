# A `_` in a call's bracket leaves an origin binder to the call's one solve,
# which binds it from the argument's place beside the value binders the
# bracket binds, positionally or by keyword.


def first[o: ImmOrigin, n: Int](p: Pointer[Int, o]) -> Int:
    return p[] + n


def main():
    var y = 2
    print(first[_, 7](Pointer(to=y)))
    print(first[o=_, n=7](Pointer(to=y)))
    print(first[_, n=1](Pointer(to=y)))
    var z = 5
    var p = Pointer(to=z)
    print(first[_, 3](p))

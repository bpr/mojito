# A `_` in a function's bracket leaves an origin binder unbound in its
# declared slot, beside the value binders the bracket binds, and a call of
# the bound function infers it from the argument's place.


def first[o: ImmOrigin, n: Int](p: Pointer[Int, o]) -> Int:
    return p[] + n


def pair[o: ImmOrigin, n: Int, m: Int](p: Pointer[Int, o]) -> Int:
    return p[] * n + m


comptime plus_six = first[_, 6]
comptime scaled = pair[_, 10, 3]


def main():
    var y = 2
    print(plus_six(Pointer(to=y)))
    print(scaled(Pointer(to=y)))

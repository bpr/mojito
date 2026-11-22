# expect: write through a Pointer whose origin is immutable
# A `Pointer[T, o]` over an `o: ImmOrigin` binder is immutable over what
# the binder binds, even when the argument pointer is mutable: the result of
# `same(Pointer(to=x))` takes no store.
def same[o: ImmOrigin](p: Pointer[Int, o]) -> Pointer[Int, o]:
    return p


def main():
    var x = 7
    var q = same(Pointer(to=x))
    q[] = 8
    print(x)

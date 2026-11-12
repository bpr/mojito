# expect: a mutable-origin pointer for parameter 'p' of 'bump'
# A binder typed by an alias of `Origin[mut=True]` is a mutable origin
# binder: an `ImmOrigin(o)` pointer cannot fill it.
comptime M = Origin[mut=True]


def bump[o: M](p: Pointer[Int, o]):
    p[] += 1


def main():
    var x = 3
    var q: Pointer[Int, ImmOrigin(origin_of(x))] = Pointer(to=x)
    bump(q)
    print(x)

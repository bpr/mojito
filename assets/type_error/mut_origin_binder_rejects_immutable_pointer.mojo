# expect: a mutable-origin pointer for parameter 'p' of 'bump'
# An `ImmOrigin(o)` pointer cannot fill a free function's `MutOrigin`
# binder.
def bump[o: MutOrigin](p: Pointer[Int, o]):
    p[] += 1


def main():
    var x = 3
    var q: Pointer[Int, ImmOrigin(origin_of(x))] = Pointer(to=x)
    bump(q)
    print(x)

# A free function's own origin binder resolves as a `Pointer` origin
# argument in its signature and binds from the argument at the call:
# a parametric-mutability binder and a `MutOrigin` binder that writes.
def peek[m: Bool, //, o: Origin[mut=m]](p: Pointer[Int, o]) -> Int:
    return p[]


def bump[o: MutOrigin](p: Pointer[Int, o]):
    p[] += 1


def main():
    var x = 3
    print(peek(Pointer(to=x)))
    bump(Pointer(to=x))
    print(x)

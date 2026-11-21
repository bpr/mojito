# expect: invalid call to 'f': value passed to 'p' cannot be converted from 'Pointer[Int, ImmUntrackedOrigin]' to 'Pointer[Int, o]'
# An infer-only `m` the bracket binds by keyword fixes the mutability of the
# origin `o` it governs before the arguments bind `o`, so an immutable
# pointer does not convert to `Pointer[Int, o]` at `m=True` (pinned Mojo:
# the same words).
def f[m: Bool, //, o: Origin[mut=m]](p: Pointer[Int, o]) -> Int:
    return p[]


def g(q: Pointer[Int, ImmUntrackedOrigin]) -> Int:
    return f[m=True](q)


def main():
    var x = 3
    print(g(Pointer(to=x).as_imm().unsafe_origin_cast[ImmUntrackedOrigin]()))
    _ = x

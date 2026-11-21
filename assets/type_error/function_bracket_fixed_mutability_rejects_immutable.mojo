# expect: invalid call to 'f': value passed to 'p' cannot be converted from 'Pointer[Int, ImmUntrackedOrigin]' to 'Pointer[Int, o]'
# A call's bracket binds its explicit parameters before the arguments:
# `t=MutUntrackedOrigin` fixes the shared `m` to `True`, so the origin `o`
# the argument binds must be mutable, and an immutable pointer does not
# convert (pinned Mojo: the same words).
def f[m: Bool, //, o: Origin[mut=m], t: Origin[mut=m]](p: Pointer[Int, o]) -> Int:
    return p[]


def g(q: Pointer[Int, ImmUntrackedOrigin]) -> Int:
    return f[t=MutUntrackedOrigin](q)


def main():
    var x = 3
    print(g(Pointer(to=x).as_imm().unsafe_origin_cast[ImmUntrackedOrigin]()))
    _ = x

# A `ref` binding whose referent is a `Pointer` reads the pointer through
# the binding: a subscript of the binding dereferences the pointer it
# designates, a `mut` pointer parameter reads the caller's pointer, and a
# `ref` of such a binding designates the same pointer.
def bump[o: Origin[mut=True]](mut p: Pointer[Int, o]):
    p[] = 5


def main():
    var x = 4
    var p = Pointer(to=x)
    ref s = p
    s[] = 7
    print(p[], x)
    bump(p)
    print(x)
    ref r = s
    print(r[] + 1)
    var t = (Pointer(to=x), 1)
    ref u = t[0]
    print(u[])

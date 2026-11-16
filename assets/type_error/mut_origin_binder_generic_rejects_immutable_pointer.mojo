# expect: a mutable-origin pointer for parameter 'p' of 'f'
# A generic function's `MutOrigin` binder rejects a pointer into a
# read-only argument, as the concrete path does.
def f[T: AnyType, o: MutOrigin](p: Pointer[T, o]):
    pass


def g(x: Int):
    f(Pointer(to=x))


def main():
    var x = 1
    g(x)

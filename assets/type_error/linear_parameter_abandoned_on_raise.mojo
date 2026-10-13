# expect: 't' abandoned without being explicitly destroyed: unhandled explicitly destroyed type 'AnyType'
# An owned parameter typed by a type parameter whose bounds do not prove
# `Deinitable` is linear: a raise before it is moved on abandons it.
# The pinned Mojo rejects it too.
def boom(n: Int) raises -> Int:
    if n > 5:
        raise Error("boom")
    return n

def g[T: Movable](var t: T, n: Int) raises -> T:
    _ = boom(n)
    return t^

def main() raises:
    print(g(1, 2))

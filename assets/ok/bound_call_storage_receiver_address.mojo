# A call through a `def(...)` bound passes its callee's storage by address,
# as Mojo's requirement call takes a reference to its `read self`: a
# capturing lambda whose body writes a `var` capture writes the caller's
# storage, so the second call sees the first's write and the local sees
# both. A function value bound to the same parameter is read through that
# reference, and a register-passable storage starts each call from the
# value it captured.
def twice[F: def() -> Int](f: F) -> Int:
    return f() * 10 + f()


def apply[F: def(Int) -> Int](f: F, x: Int) -> Int:
    return f(x) + f(x)


def dbl(x: Int) -> Int:
    return x * 2


def main():
    var s = String("ab")
    var g = lambda {var s^} -> Int: (s.__iadd__("!"), s.byte_length())[1]
    print(twice(g))
    print(g())
    print(apply(dbl, 3))
    var n = 1
    print(apply(lambda (x: Int) {var n^} -> Int: (n.__iadd__(x), n)[1], 10))

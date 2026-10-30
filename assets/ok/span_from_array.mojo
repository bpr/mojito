# `Span`'s `@implicit` constructor from an `Array`: a list display and an
# `Array` binding convert at a `Span` parameter, and a compile-time `Span`
# built over a display binds a value parameter and folds `len`. The
# constructor's `array.unsafe_ptr()` carries `Span`'s own origin binder, and
# an overload whose parameter type names a value binder is called by the
# name it declares.


def k(s: Span[Int, _]):
    print(len(s), s[len(s) - 1])


def g[c: Span[Int, _]]():
    print(len(c), c[1])


struct Width:
    var n: Int

    def __init__(out self, a: List[Int]):
        self.n = -len(a)

    def __init__[w: Int](out self, a: Array[Int, w]):
        self.n = w

    def __init__(out self, a: Array[Float64, _]):
        self.n = a.length * 10


def count(a: List[Int]) -> Int:
    return -1


def count(a: Array[Int, _]) -> Int:
    return a.length


def main():
    k([4, 5])
    var a = [1, 2, 3]
    k(a)
    var s = Span(a)
    s[0] = 7
    print(len(s))
    print(a[0])
    g[Span([4, 5, 6])]()
    comptime s4 = Span([1, 2, 3, 4])
    comptime n = len(s4)
    print(n)
    var l: List[Int] = [1, 2]
    var f: Array[Float64, 2] = [1.0, 2.0]
    print(Width(a).n, Width(f).n, Width(l).n)
    print(count(a), count(l))

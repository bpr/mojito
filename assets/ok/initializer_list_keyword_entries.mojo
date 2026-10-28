# An initializer list's keyword entries are the construction's keyword
# arguments, as in current Mojo: `{1, kw=2}` at a contextual `P` is
# `P(1, kw=2)`, in an argument, a return, an annotated binding, and a
# `comptime` binding at module or function scope. A `comptime` brace
# annotated with a non-collection type is that type's initializer list,
# never a set display, and a struct with an origin parameter takes the
# annotation's origin.
struct P(ImplicitlyCopyable, Movable):
    var a: Int
    var b: Int

    def __init__(out self, a: Int = 1, kw: Int = 7):
        self.a = a
        self.b = kw


@fieldwise_init
struct Pair[T: ImplicitlyCopyable & Deinitable & Writable](Copyable, Movable):
    var first: Self.T
    var second: Self.T

    def swapped(self) -> Self:
        return {first = self.second, second = self.first}


@fieldwise_init
struct Holder(Movable):
    var xs: List[Int]

    def view(ref self) -> Span[Int, origin_of(self.xs)]:
        return {unsafe_ptr = self.xs.unsafe_ptr(), length = 2}


comptime MODULE_PAIR: Tuple[Int, Int] = {3, 4}
comptime MODULE_P: P = {5, kw=6}


def take(p: P) -> Int:
    return p.a * 10 + p.b


def make(n: Int) -> P:
    return {n, kw=n + 1}


def pair[T: ImplicitlyCopyable & Deinitable & Writable](a: T, b: T) -> Pair[T]:
    return {a, second=b}


def twice[n: Int]() -> Int:
    comptime t: Tuple[Int, Int] = {n, n * 2}
    return t[0] + t[1]


def main():
    print(take({1, kw=2}), take({kw=3}), take({}))
    var p: P = {a=8, kw=9}
    print(p.a, p.b, make(4).b)
    var q = pair(1, 2).swapped()
    print(q.first, q.second)
    var s: Pair[String] = {first="a", second="b"}
    print(s.first, s.second)
    comptime local_pair: Tuple[Int, Int] = {1, 2}
    comptime local_p: P = {1, kw=2}
    print(local_pair[0], local_pair[1], local_p.a, local_p.b)
    print(MODULE_PAIR[0], MODULE_PAIR[1], MODULE_P.a, MODULE_P.b)
    print(twice[5]())
    var xs = [1, 2, 3]
    var view: Span[Int, origin_of(xs)] = {unsafe_ptr = xs.unsafe_ptr(), length = 2}
    print(len(view), view[1], len(Holder([4, 5, 6]).view()))

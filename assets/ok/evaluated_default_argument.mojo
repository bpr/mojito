# Omitted arguments whose default is an evaluated expression: MIR lowers the
# default as a zero-parameter function, and both backends call it at the call
# site that leaves the slot out. A borrowed slot's value is the caller's
# temporary; a `var` slot's value transfers to the callee.
@fieldwise_init
struct P(Copyable):
    var x: Int
    var name: String


struct Owner:
    var tag: String

    def __init__(out self, tag: String):
        self.tag = tag

    def __deinit__(deinit self):
        print("drop", self.tag)


struct G[T: ImplicitlyCopyable & Deinitable]:
    var v: Self.T

    def __init__(out self, v: Self.T):
        self.v = v

    def show(self, prefix: String = String("pre") + String("fix")) -> String:
        return prefix

    def __getitem__(self, i: Int, suffix: String = String("su") + String("f")) -> String:
        return suffix


def f(s: String = String("a")) -> String:
    return s


def g(var s: String = String("b") + String("c")) -> String:
    s += "!"
    return s


def h(n: Int, p: P = P(3, String("pp")), k: Int = String("four").byte_length()) -> Int:
    print(p.name)
    return n + p.x + k


def take(var r: Owner = Owner(String("owned"))) -> Int:
    print("take", r.tag)
    return 2


def gen[T: ImplicitlyCopyable](x: T, s: String = String("g") + String("h")) -> T:
    print(s)
    return x


def rz(s: String = String("r")) raises -> String:
    if s.byte_length() > 5:
        raise Error("long")
    return s


def main() raises:
    print(f())
    print(f(String("z")))
    print(g())
    print(g())
    print(h(1))
    print(h(1, k=10))
    print(take())
    print(gen(3))
    print(gen(String("q")))
    print(rz())
    var held = G[Int](1)
    print(held.show())
    print(held[0])

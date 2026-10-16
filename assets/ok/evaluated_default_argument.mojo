# Omitted arguments whose default is an evaluated expression: MIR lowers the
# default as a zero-parameter function, and both backends call it at the call
# site that leaves the slot out, a method's, static method's,
# constructor's, or nested `def`'s too, under the struct's parameters, the
# enclosing function's, and its own. A borrowed slot's value is the caller's
# temporary, destroyed when the call returns (`use` prints `drop dflt`,
# `hold` prints `drop method`, `plain` prints `drop nested`); a `var` slot's
# value transfers to the callee.
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


struct Box[n: Int]:
    var v: Int

    def __init__(out self, v: Int, scale: Int = Self.n * 2):
        self.v = v * scale

    def get[m: Int](self, x: Int = Self.n + m) -> Int:
        return self.v + x

    def hold(self, r: Owner = Owner(String("method"))) -> Int:
        print("hold", r.tag)
        return self.v

    @staticmethod
    def make(k: Int = Self.n * 10) -> Int:
        return k


def f(s: String = String("a")) -> String:
    return s


def g(var s: String = String("b") + String("c")) -> String:
    s += "!"
    return s


def h(n: Int, p: P = P(3, String("pp")), k: Int = String("four").byte_length()) -> Int:
    print(p.name)
    return n + p.x + k


def use(r: Owner = Owner(String("dflt"))) -> Int:
    print("use", r.tag)
    return 1


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


def outer[n: Int](k: Int) -> Int:
    var base = k

    def inner(x: Int, y: Int = n * 3, z: Int = n + 1) {imm base} -> Int:
        return base + x + y * z

    def plain(r: Owner = Owner(String("nested"))) -> Int:
        print("plain", r.tag)
        return 1

    def owned(var r: Owner = Owner(String("nested owned"))) -> Int:
        print("owned", r.tag)
        return 2

    return plain() + owned() + inner(1) + inner(1, 2) + inner(1, z=4)


def main() raises:
    print(f())
    print(f(String("z")))
    print(g())
    print(g())
    print(h(1))
    print(h(1, k=10))
    print(take())
    print(use())
    print(gen(3))
    print(gen(String("q")))
    print(rz())
    var held = G[Int](1)
    print(held.show())
    print(held[0])
    var b = Box[2](1)
    print(b.get[5]())
    print(b.hold())
    print(Box[4].make())
    print(outer[2](10))

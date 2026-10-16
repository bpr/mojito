# Omitted arguments are filled at the call, for every call shape, as
# upstream's call binding writes the callee's default into the call: the
# lowering of each checked call records its callee, and once every
# declaration is lowered the fill writes each omitted argument into the
# call, under the struct's parameters, the enclosing function's, and the
# callee's own. A default that is neither a literal nor a conversion of one
# is a zero-parameter default function the call runs. A borrowed slot's
# value is the caller's temporary, destroyed when the call returns (`use`
# prints `drop dflt`, `hold` prints `drop method`, `plain` prints `drop
# nested`, `through` called through a value prints `drop value`, `a[1]`
# prints `drop sub`, `sib` calling `plain` through its capture prints `drop
# nested`, and `g` rebound to `use` prints `drop dflt`); a `var` slot's
# value transfers to the callee. A literal default is filled the same way,
# so a positional-only slot after an omitted one is passed by position
# (`plit[1, 2]()`), and a requirement's `None` default reaches a conformer
# called through its bound (`go(a)`).
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


trait Shows:
    def show(self, x: Optional[Int] = None) -> Int:
        ...


struct A(Shows):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def show(self, x: Optional[Int] = None) -> Int:
        if x:
            return self.v + x.value()
        return self.v

    def __getitem__(self, i: Int, r: Owner = Owner(String("sub"))) -> Int:
        print("getitem", r.tag)
        return self.v + i


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


def go[T: Shows](x: T) -> Int:
    return x.show()


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


def through(r: Owner = Owner(String("value"))) -> Int:
    print("through", r.tag)
    return 3


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


def plit[n: Int, m: Int](x: Int = 1, y: Int = m * 2, /) -> Int:
    return n + x + y


def outer[n: Int](k: Int) -> Int:
    var base = k

    def inner(x: Int, y: Int = n * 3, z: Int = n + 1) {imm base} -> Int:
        return base + x + y * z

    def plain(r: Owner = Owner(String("nested"))) {imm base} -> Int:
        print("plain", r.tag)
        if base > 0:
            return 1
        return 0

    def owned(var r: Owner = Owner(String("nested owned"))) -> Int:
        print("owned", r.tag)
        return 2

    def sib() {imm plain} -> Int:
        return plain()

    return plain() + owned() + inner(1) + inner(1, 2) + inner(1, z=4) + sib()


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
    var called = through
    print(called())
    var a = A(5)
    print(a[1])
    print(go(a))
    var rebound = use
    rebound = use
    print(rebound())
    print(plit[1, 2]())
    print(plit[1, 2](10))

# An annotated `comptime` binding whose value is not of the annotation's
# struct type converts it through the struct's `@implicit` constructor, as
# at the pin: `comptime w: L = 4` binds `L(4)`, module-level and local, in
# a plain body and in a generic one.
struct L(ImplicitlyCopyable, Writable):
    var v: Int

    @implicit
    def __init__(out self, value: IntLiteral, /):
        self.v = value

    def write_to(self, mut w: Some[Writer]):
        w.write("L(", self.v, ")")


struct W(ImplicitlyCopyable):
    var n: Int

    @implicit
    def __init__(out self, n: Bool):
        self.n = 7 if n else 0


def f[w: L]():
    print(w)


def name[s: StaticString]() -> String:
    return String(s)


comptime MOD: L = 4
comptime FLAG: W = True
comptime TEXT: StaticString = "mod"
comptime NOTHING: Optional[Int] = None


def g[n: Int]():
    comptime w: L = 9
    f[w]()
    print(w.v + n)


def main():
    comptime w: L = 5
    f[w]()
    f[MOD]()
    print(MOD, w)
    var x = MOD
    print(x.v, w.v, FLAG.n)
    g[1]()
    comptime t: StaticString = "loc"
    print(name[t](), name[TEXT]())
    comptime if NOTHING:
        print("some")
    else:
        print("none")

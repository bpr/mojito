# A subscript is its getter's call, so over a by-value `__getitem__` the
# result is an owned temporary, not a place: a read argument, an operator
# operand, a method receiver, a field read, and a discarded statement each
# destroy it once their call returns, and a `var` parameter, a `var`
# binding, `_ =`, `^`, `.copy()`, and a chain middle take it without a copy.
struct S(Copyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v
        print("copy", self.v)

    def __deinit__(deinit self):
        print("del", self.v)

    def __lt__(self, other: Self) -> Bool:
        return self.v < other.v

    def __neg__(self) -> Self:
        return S(-self.v)

struct B:
    def __init__(out self):
        pass

    def __getitem__(self, i: Int) -> S:
        return S(i)

def f(s: S):
    print("f", s.v)

def g(var s: S):
    print("g", s.v)

def main():
    var b = B()
    f(b[1])
    print("--")
    print(b[2] < b[3])
    print("--")
    print(b[4].__lt__(b[5]))
    print("--")
    print(b[6].v)
    print("--")
    print(b[14].v, b[15].v)
    print("--")
    b[13]
    print("--")
    g(b[9])
    print("--")
    var t = b[10]
    print(t.v)
    print("--")
    _ = b[11]
    print("--")
    var u = b[12]^
    print(u.v)
    print("--")
    var c = b[1].copy()
    print(c.v)
    print("--")
    print(S(0) < b[2] < S(5))
    print("--")
    -b[16]
    print("end")

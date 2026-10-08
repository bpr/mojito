# Local `comptime` bindings a generic body's served `comptime for` holds,
# decided per instance by the template: an annotated `Int` or `Bool`
# binding, a comparison, an element of a named compile-time list (local or
# module) or of a literal display at the index. A read past a display in
# an arm the instance never takes does not fail it, and a local `comptime`
# list of one `def` is not the same-named binding of a later one. A local
# type alias of a binder (`comptime U = Self.T`) keys a `comptime if`.
comptime NAMES = ["a", "b"]


struct S[n: Int]:
    def __init__(out self):
        pass

    def f(self):
        comptime for i in range(Self.n):
            comptime m: Int = i * Self.n
            comptime b: Bool = i > 0
            print(m, b)

    def g(self):
        comptime for i in range(Self.n):
            comptime s = NAMES[i]
            comptime q = [10, 20][i]
            print(s, q)

    def h(self):
        comptime if Self.n < 3:
            comptime for i in range(Self.n):
                comptime q = [1, 2][i]
                print(q)
        else:
            print("big")


struct Box[T: Copyable & Deinitable & Writable]:
    var v: Self.T

    def __init__(out self, var v: Self.T):
        self.v = v^

    def local_ty(self) -> Int:
        comptime U = Self.T
        comptime if U == Int:
            return 1
        return 0


def alias_kind[T: Copyable & Writable](x: T) -> Int:
    comptime U = T
    comptime if U == Int:
        return 1
    return 0


def first[n: Int]():
    comptime xs = ["x", "y"]
    comptime for i in range(n):
        comptime s = xs[i]
        print(s)


def second[n: Int]():
    comptime xs = ["x", String(n)]
    comptime for i in range(n):
        comptime s = xs[i]
        print(s)


def main():
    S[2]().f()
    S[2]().g()
    S[2]().h()
    S[5]().h()
    first[2]()
    second[2]()
    print(Box(1).local_ty(), Box("a").local_ty())
    print(alias_kind(1), alias_kind("a"))

# A `comptime for` display whose element calls a method is served by the
# template like a display calling a module `def`: source validation types
# the element with the binders symbolic, and an element it types as an
# `Int`, `Bool`, `Float64`, or string is lifted with the display and
# evaluated per instance. The receiver may be a construction, a type, a
# generic struct's instance type, `Self`, or a local `comptime` binding.
@fieldwise_init
struct P(Copyable, Movable, ImplicitlyCopyable):
    var v: Int

    def get(self) -> Int:
        return self.v

    def half(self) -> Float64:
        return Float64(self.v) / 2


struct S:
    @staticmethod
    def g(x: Int) -> Int:
        return x + 10


struct K[k: Int]:
    @staticmethod
    def g(x: Int) -> Int:
        return x + Self.k


struct T[n: Int]:
    @staticmethod
    def g(x: Int) -> Int:
        return x * 6

    @staticmethod
    def show():
        comptime for x in [Self.g(Self.n), Self.n]:
            print(x)


def instance[n: Int]():
    comptime for p in [P(n).get(), n]:
        print(p)
    comptime for h in [P(n).half(), 0.25]:
        print(h)


def static[n: Int]():
    comptime for p in [S.g(n), n]:
        print(p)
    comptime for p in [K[n].g(n), n]:
        print(p)


def strings[n: Int]():
    comptime for s in [String(n).upper(), "x"]:
        print(s)


def local[n: Int]():
    comptime q = P(n)
    comptime for p in [q.get() + 1, n]:
        print(p)


def main():
    instance[3]()
    static[3]()
    strings[3]()
    local[3]()
    T[2].show()

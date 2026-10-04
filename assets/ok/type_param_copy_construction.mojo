# Constructing a type parameter through its bound's `Copyable` initializer
# (`Self.T(copy=x)`, `T(copy=x)`) copies the source as the bound type does: a
# value read for a built-in, the struct's own copy initializer otherwise, in
# the template and in each instance clone.

struct Counter(Copyable, Defaultable):
    var n: Int

    def __init__(out self):
        self.n = 0

    def __init__(out self, *, copy: Self):
        print("copying", copy.n)
        self.n = copy.n + 100


struct Box[T: Copyable & Deinitable & Defaultable](Copyable):
    var x: Self.T

    def __init__(out self, var x: Self.T):
        self.x = x^

    def __init__(out self, *, copy: Self):
        self.x = Self.T(copy=copy.x)

    def get(self) -> Self.T:
        return Self.T(copy=self.x)

    def get_cloned(self) -> Self.T:
        comptime if Self.T == Int:
            return Self.T(copy=self.x)
        else:
            return Self.T(copy=self.x)


def dup[T: Copyable](x: T) -> T:
    return T(copy=x)


def main():
    var b = Box[Int](5)
    print(b.get(), b.get_cloned())
    var c = Box[Counter](Counter())
    print(c.get().n)
    print(c.get_cloned().n)
    var c2 = c.copy()
    print(c2.x.n)
    print(dup(3.5))
    print(dup(String("s")))
    print(dup(Counter()).n)
    print(Int(copy=7))

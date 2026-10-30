# A compile-time parameter shadows a function of the same name across its
# declaration, whether the function is the prelude's (`abs`, `index`) or
# the module's own, and a method's parameter shadows it in the method.
def foo() -> Int:
    return 0


def g[abs: Int]() -> Int:
    return abs + 1


def h[foo: Int]() -> Int:
    var y = foo
    return y + 1


def pick[index: Int](values: List[Int]) -> Int:
    return values[index]


def first_index[I: Indexer, //](i: I) -> Int:
    return index(i)


struct S[abs: Int](Copyable):
    var x: Int

    def __init__(out self):
        self.x = Self.abs

    def get[foo: Int](self) -> Int:
        if foo < Self.abs:
            return foo + self.x
        return 0


def main():
    print(g[3](), h[3](), pick[1]([4, 5, 6]))
    print(first_index(3), index(4), first_index(UInt8(5)))
    var s = S[5]()
    print(s.get[2](), s.get[7]())

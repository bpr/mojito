# A value pack typed by an earlier type parameter (`*vs: T`), the shape of
# upstream's `ParameterList`: `T` is solved from the elements, or from a
# forwarded pack's element type.


def count[T: ImplicitlyCopyable & Writable, //, *vs: T]() -> Int:
    return len(vs)


def first[T: ImplicitlyCopyable & Writable, //, *vs: T]() -> T:
    return vs[0]


def each[T: ImplicitlyCopyable & Deinitable & Writable, //, *vs: T]():
    comptime for i in range(len(vs)):
        var x: T = vs[i]
        print(x)


struct P[T: ImplicitlyCopyable & Writable, //, *vs: T]:
    def __init__(out self):
        pass

    def count(self) -> Int:
        return len(Self.vs)

    def at0(self) -> Self.T:
        return Self.vs[0]


def forward_ints[*xs: Int]() -> Int:
    return P[*xs]().count()


def forward_bools[*xs: Bool]() -> Int:
    return P[*xs]().count()


def main():
    print(count[1, 2, 3](), count[True, False]())
    print(first[5, 6](), first[2.5]())
    each[1, 2, 3]()
    print(P[7, 8]().count(), P[7, 8]().at0())
    print(forward_ints[1, 2, 3](), forward_bools[True]())

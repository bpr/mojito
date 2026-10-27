# A parameter binder's annotation resolves like any type expression: a
# trait makes a type parameter, and any other type a value parameter of that
# type, whether a sized scalar (`Int16`), an alias of a type (`MyInt`), or a
# struct's binder (`i8: Int8`).

comptime MyInt = Int


def aliased[n: MyInt]() -> Int:
    return n


def sized[v: Int16]() -> Int16:
    return v + 1


def wide[x: UInt8, y: UInt64, z: Float32]():
    print(x, y, z)


def defaulted[x: Int8 = 3]() -> Int8:
    return x


def doubled(n: Int) -> Int8:
    return Int8(n * 2)


def show[x: Int8]():
    print(x)


def forwarded[n: Int]():
    show[doubled(n)]()


def summed[*vs: Int16]() -> Int16:
    var total: Int16 = 0
    comptime for v in vs:
        total += v
    return total


struct S[i8: Int8]:
    @staticmethod
    def num() -> Int8:
        return Self.i8


def main():
    print(aliased[3]())
    print(sized[5]())
    wide[200, 9, 1.5]()
    print(defaulted(), defaulted[x=4]())
    forwarded[3]()
    print(summed[1, 2, 3]())
    print(S[3].num())

# A `def` keyed on a variadic value pack is served by its template: the body
# is checked once, with each `len(values)`, `values.__len__()`, and
# `values[i]` the parameter constant the elaborator folds per instance.


def weighted[*values: Int]() -> Int:
    var total = 0
    comptime for i in range(len(values)):
        total += values[i] * (i + 1)
    return total


def count[*flags: Bool]() -> Int:
    return flags.__len__()


def first_is_float32[*kinds: DType]() -> Bool:
    return kinds[0] == DType.float32


def scaled[*values: Int]() -> Int:
    comptime n = len(values)
    return n * 10 + values[n - 1]


def tagged[T: Writable, *values: Int](x: T) -> Int:
    print(x)
    return values[len(values) - 1]


def forwarded[n: Int]() -> Int:
    return tagged[Int, n, n + 1](n)


@fieldwise_init
struct Base:
    var k: Int

    def shifted[*values: Int](self) -> Int:
        return self.k + len(values) + values[1]

    @staticmethod
    def arity[*values: Int]() -> Int:
        return values.__len__()


def main():
    print(weighted[1, 2, 3]())
    print(weighted[7]())
    print(count[True, False, True]())
    print(first_is_float32[DType.float32]())
    print(first_is_float32[DType.int8, DType.float32]())
    print(scaled[4, 5]())
    print(tagged[String, 3, 9]("x"))
    print(forwarded[5]())
    print(Base(10).shifted[3, 4, 5]())
    print(Base.arity[1, 2]())

# A `def` declared in a `comptime for` that calls a sibling declared in the
# same loop reads the loop's binder through that call when the sibling kept
# it, as upstream's `RemoveUnusedParams` reads a lifted closure's call of a
# generator with that generator's unused parameters removed: `mid` keeps one
# instance per index when `leaf` reads the index, through an argument's type
# or its body, and shares one when `leaf` reads neither.


@fieldwise_init
struct Lane[n: Int](Copyable, ImplicitlyCopyable, Movable):
    var x: Int


@fieldwise_init
struct Arr[n: Int](Copyable, Movable):
    var v: Lane[Self.n]

@no_inline
def forwards(x: Int) -> Int:
    var total = 0

    comptime for i in range(2):

        @no_inline
        def leaf(a: Arr[i + 1], y: Int) -> Int:
            return y + i

        @no_inline
        def mid(a: Arr[i + 1], y: Int) -> Int:
            return leaf(a, y)

        total += mid(Arr[i + 1](Lane[i + 1](1)), x)
    return total


@no_inline
def forwards_reads(x: Int) -> Int:
    var total = 0

    comptime for i in range(2):

        @no_inline
        def leaf(a: Arr[i + 1], y: Int) -> Int:
            return a.v.x + y + i

        @no_inline
        def mid(a: Arr[i + 1], y: Int) -> Int:
            return leaf(a, y)

        total += mid(Arr[i + 1](Lane[i + 1](1)), x)
    return total


@no_inline
def no_read(x: Int) -> Int:
    var total = 0
    comptime for i in range(2):
        @no_inline
        def leaf(a: Arr[i + 1], y: Int) -> Int:
            return y + 1
        @no_inline
        def mid(a: Arr[i + 1], y: Int) -> Int:
            return leaf(a, y)
        total += mid(Arr[i + 1](Lane[i + 1](1)), x)
    return total


@no_inline
def reads_a(x: Int) -> Int:
    var total = 0
    comptime for i in range(2):
        @no_inline
        def leaf(a: Arr[i + 1], y: Int) -> Int:
            return a.v.x + y
        @no_inline
        def mid(a: Arr[i + 1], y: Int) -> Int:
            return leaf(a, y)
        total += mid(Arr[i + 1](Lane[i + 1](1)), x)
    return total


@no_inline
def vector(x: Int) -> Int:
    var total = 0
    comptime for i in range(2):
        @no_inline
        def leaf(y: SIMD[DType.int64, i + 1]) -> Int:
            return Int(y.reduce_add())
        @no_inline
        def mid(y: SIMD[DType.int64, i + 1]) -> Int:
            return leaf(y)
        total += mid(SIMD[DType.int64, i + 1](x))
    return total


@no_inline
def deeper(x: Int) -> Int:
    var total = 0
    comptime for i in range(2):
        @no_inline
        def leaf(a: Arr[i + 1], y: Int) -> Int:
            return y * (i + 1)
        @no_inline
        def mid(a: Arr[i + 1], y: Int) -> Int:
            return leaf(a, y)
        @no_inline
        def top(a: Arr[i + 1], y: Int) -> Int:
            return mid(a, y) + 1
        total += top(Arr[i + 1](Lane[i + 1](1)), x)
    return total


@no_inline
def generic_param[n: Int](x: Int) -> Int:
    var total = 0
    comptime for i in range(2):
        @no_inline
        def leaf(a: Arr[n + i], y: Int) -> Int:
            return y + n + i
        @no_inline
        def mid(a: Arr[n + i], y: Int) -> Int:
            return leaf(a, y)
        total += mid(Arr[n + i](Lane[n + i](1)), x)
    return total


def main():
    print(forwards(5), forwards_reads(5))
    print(no_read(5), reads_a(5), vector(5), deeper(5))
    print(generic_param[1](5), generic_param[3](5))

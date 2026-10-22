# A slot of a generic struct is split and promoted as any other: `sroa`
# types each field slot at the slot type's arguments, so `Arr[n]`'s field
# slot is a `SIMD[DType.int64, n]` over the enclosing generator's `n`, and
# `mem-2-reg` promotes it, as upstream's `SROA` and `Mem2Reg` take the
# allocation whatever its element types name. A dead construction through
# an `@always_inline` constructor then leaves no read of the generator's
# parameter, so `plain_nested[1/2/4]` is one instance, as on the pin; so is
# `outer_nested`, whose `Outer[n]` holds an `Arr[n]` and splits over two
# rounds. A trivially register-passable `V[n]` copies as its value, and a
# `P[Int]` over a field `T` splits at its argument.
struct Arr[n: Int](Copyable, Movable):
    var v: SIMD[DType.int64, Self.n]

    @always_inline
    def __init__(out self, v: SIMD[DType.int64, Self.n]):
        self.v = v


struct Outer[n: Int](Copyable, Movable):
    var arr: Arr[Self.n]
    var k: Int

    @always_inline
    def __init__(out self, var arr: Arr[Self.n], k: Int):
        self.arr = arr^
        self.k = k


@fieldwise_init
struct V[n: Int](TrivialRegisterPassable):
    var lanes: SIMD[DType.int32, Self.n]


struct P[T: Copyable & Deinitable](Copyable, Movable):
    var x: Self.T

    @always_inline
    def __init__(out self, x: Self.T):
        self.x = x.copy()


def plain_nested[n: Int](y: Int) -> Int:
    def inner(a: Arr[n], y: Int) -> Int:
        return y + 1

    return inner(Arr[n](SIMD[DType.int64, n](1)), y)


def outer_nested[n: Int](y: Int) -> Int:
    def inner(o: Outer[n], y: Int) -> Int:
        return y * 2

    return inner(Outer[n](Arr[n](SIMD[DType.int64, n](3)), 5), y)


def lanes_sum[n: Int](y: Int32) -> Int32:
    var v = V[n](SIMD[DType.int32, n](y))
    var w = v
    return w.lanes.reduce_add()


def concrete() -> Int:
    var p = P[Int](4)
    p.x += 1
    return p.x


def main():
    print(plain_nested[1](1), plain_nested[2](2), plain_nested[4](3))
    print(outer_nested[1](1), outer_nested[2](2), outer_nested[4](3))
    print(lanes_sum[4](3), lanes_sum[8](2))
    print(concrete())

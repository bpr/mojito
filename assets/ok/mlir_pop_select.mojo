# Upstream's `std.utils._select`: `_select_register_value` is one
# `pop.select` over a `Bool`'s `!kgen.scalar<bool>` storage, choosing one
# of two trivially register-passable values without a branch.
from std.utils._select import _select_register_value


@fieldwise_init
struct Point(TrivialRegisterPassable, Writable):
    var x: Int
    var y: Float64


@always_inline("builtin")
def pick(condition: Bool, a: Int, b: Int) -> Int:
    return __mlir_op.`pop.select`(condition.__mlir_bool__(), a, b)


@fieldwise_init
struct Tag[k: Int](TrivialRegisterPassable):
    pass


def tagged[n: Int](tag: Tag[pick(n > 2, n, 0)]) -> Int:
    return n * 10


def clamped[n: Int]() -> Int:
    comptime m = _select_register_value(n > 2, n, 0)
    return m


def main():
    print(_select_register_value(True, 1, 2), _select_register_value(False, 1, 2))
    var p = _select_register_value(False, Point(1, 2.5), Point(3, 4.5))
    print(p.x, p.y)
    print(_select_register_value(True, Float64(1.5), Float64(2.5)))
    print(
        _select_register_value(
            False, SIMD[DType.int8, 4](1), SIMD[DType.int8, 4](2)
        )
    )
    print(_select_register_value(True, False, True))
    var c = 3 > 2
    print(__mlir_op.`pop.select`(c.__mlir_bool__(), UInt8(7), UInt8(9)))
    print(pick(not c, 4, 5))
    comptime x = _select_register_value(True, Int(4), Int(5))
    print(x, clamped[3](), clamped[1]())
    print(tagged[1](Tag[0]()), tagged[5](Tag[5]()))

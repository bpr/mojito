# A struct's parameterized `comptime` member read through the struct, in
# type position and in a compile-time expression.
from std.builtin.variadics import _MLIR


struct P:
    comptime Two[a: Int, b: Int] = a * 10 + b
    comptime sz[T: AnyType] = 3
    comptime Pair[A: AnyType, B: AnyType] = Tuple[Int, Int]
    comptime Buf[n: Int] = SIMD[DType.int64, n]


struct S[T: AnyType]:
    comptime Of[U: AnyType] = Tuple[Int, Int]


struct K[T: AnyType]:
    comptime __getitem_param__[i: Int] = Self.T

    def __init__(out self):
        pass


struct M:
    comptime KGENTypeType = __mlir_type.`!kgen.type`
    comptime ListOf[elt_type: Self.KGENTypeType] = __mlir_type[
        `!kgen.param_list<`, elt_type, `>`
    ]
    comptime Of[T: AnyType] = K[T]


struct R:
    comptime Same[T: AnyType] = P.Pair[T, T]


def width[n: Int](x: P.Buf[n]) -> Int:
    return n


def size[T: AnyType]() -> Int:
    return 1


def ints[raw: _MLIR.KGENParamListType[Int]]() -> Int:
    return 7


def types[raw: M.ListOf[M.KGENTypeType]]() -> Int:
    return 8


def main():
    print(P.Two[1, 2], P.Two[a=3, b=4], P.Two[5, b=6])
    comptime three = P.sz[Int]
    print(three)
    var pair: P.Pair[Bool, Int] = (7, 8)
    print(pair[1])
    var of: S[Int].Of[Bool] = (9, 10)
    print(of[0])
    comptime Alias = S[Int].Of[Bool]
    var aliased: Alias = (11, 12)
    print(aliased[1])
    print(size[S[Int].Of[Bool]]())
    print(width[4](SIMD[DType.int64, 4](1)))
    var buf: P.Buf[2] = SIMD[DType.int64, 2](13, 14)
    print(buf[1])
    var element: M.Of[Int]()[0] = 15
    print(element)
    var same: R.Same[Bool] = (16, 17)
    print(same[1])

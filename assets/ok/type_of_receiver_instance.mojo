# A `type_of(value)` receiver is the type itself, as upstream looks a member
# up on the type value: a type with no source spelling (a `TypeList` over a
# raw list, one over a pack) reads its members, a generic struct's statics
# bind its instance, and a construction through it takes `mlir_value=` and
# `copy=` at a symbolic `SIMD`.
from std.builtin.variadics import TypeList


@fieldwise_init
struct Box[T: ImplicitlyCopyable & Writable & Deinitable, n: Int](Copyable):
    comptime size = Self.n * 2
    var v: Self.T

    @staticmethod
    def make(v: Self.T) -> Self:
        return Self(v)

    @staticmethod
    def twice[m: Int]() -> Int:
        return Self.n * m

    def get(self) -> Self.T:
        return self.v


def pack_length[*Ts: AnyType](x: TypeList[Trait=AnyType, Ts.values]) -> Int:
    return type_of(x).length


def statics[T: ImplicitlyCopyable & Writable & Deinitable, n: Int](
    b: Box[T, n]
):
    print(type_of(b).size)
    print(type_of(b).make(b.v).v)
    print(type_of(b).twice[3]())
    print(type_of(b).get(b))


def slots[dtype: DType, w: SIMDLength](x: SIMD[dtype, w]):
    print(type_of(x).dtype, Int(type_of(x).length))


def larger[
    dtype: DType, w: SIMDLength, //
](x: SIMD[dtype, w], y: SIMD[dtype, w]) -> SIMD[dtype, w]:
    return type_of(x)(mlir_value=__mlir_op.`pop.max`(x._mlir_value, y._mlir_value))


def larger_through_alias[
    dtype: DType, w: SIMDLength, //
](x: SIMD[dtype, w], y: SIMD[dtype, w]) -> SIMD[dtype, w]:
    comptime T = type_of(x)
    var v = __mlir_op.`pop.max`(x._mlir_value, y._mlir_value)
    return T(mlir_value=v)


def larger_braced[dtype: DType, //](x: SIMD[dtype, _], y: type_of(x), /) -> type_of(x):
    return {mlir_value = __mlir_op.`pop.max`(x._mlir_value, y._mlir_value)}


def copied[dtype: DType, w: SIMDLength, //](x: SIMD[dtype, w]) -> SIMD[dtype, w]:
    return type_of(x)(copy=x)


def main():
    var y = TypeList.of[Trait=AnyType, Int, Bool]()
    print(type_of(y).length)
    print(pack_length[Int, Bool, Float64](TypeList.of[Trait=AnyType, Int, Bool, Float64]()))
    statics(Box[String, 4]("hi"))
    var p = Box[Int, 2](5)
    print(type_of(p).size, type_of(p).twice[5]())
    slots(SIMD[DType.int16, 4](1))
    print(larger(Int32(3), Int32(5)))
    print(larger_through_alias(SIMD[DType.int32, 4](1, 7, 3, 4), SIMD[DType.int32, 4](2)))
    print(larger_braced(SIMD[DType.float32, 2](1.5, -2), SIMD[DType.float32, 2](0)))
    print(copied(SIMD[DType.uint8, 2](8, 9)))

# `SIMD`'s `__bool__`, `__int__`, `__mlir_index__`, `__float__`, `_refine`,
# and `__contains__` are its declared members in `std.simd`, and its
# `Boolable`, `Indexer`, and `Intable` conformances its declared trait list,
# which holds at every dtype and width; `Floatable` is not on it, as at the
# pin. A conversion, a condition, and `in` over a vector reach the members.
from std.sys import bit_width_of


def to_int[T: Intable](value: T) -> Int:
    return value.__int__()


def to_index[T: Indexer](value: T) -> Int:
    return Int(mlir_value=value.__mlir_index__())


def to_bool[T: Boolable](value: T) -> Bool:
    return value.__bool__()


def main():
    var v = SIMD[DType.int32, 4](0, 0, 3, 0)
    var zeros = SIMD[DType.float32, 2](0.0, 0.0)
    print(v.__bool__(), zeros.__bool__(), Bool(v), not v, not zeros)
    if v:
        print("nonzero")
    print(3 in v, 5 in v, 7 not in v, Int32(0) in v)
    print(Float32(2.5).__float__(), Int8(-3).__int__(), UInt8(250).__int__())
    print(Float32(2.7).__int__(), Int(mlir_value=Int32(7).__mlir_index__()))
    print(v._refine[new_size=4](), Float32(1.5)._refine[new_size=1]())
    print(to_int(Float32(3.5)), to_int(UInt16(9)), to_index(UInt8(2)))
    print(to_bool(Int8(0)), to_bool(SIMD[DType.float32, 2](0.0, 0.5)))
    print(SIMD[DType.bool, 2](0), SIMD[DType.bool, 2](2), v.eq(Int32(3)))
    print(bit_width_of[Int](), bit_width_of[DType.int16]())
    print(
        conforms_to(SIMD[DType.int32, 4], Intable),
        conforms_to(Float64, Indexer),
        conforms_to(SIMD[DType.int32, 4], Boolable),
        conforms_to(Float64, Floatable),
        conforms_to(Float32, Intable),
    )

# A `where` clause over a `DType` predicate, or any other compile-time `Bool`
# expression: the predicate's builtin body is the clause's parameter
# expression, decided at a concrete call and proven from a guarding
# `comptime if` arm or the caller's own matching clause.
def lane_kind[dtype: DType, //](x: Scalar[dtype]) -> Int where dtype.is_floating_point():
    return 1


def guarded[dtype: DType](x: Scalar[dtype]) -> Int:
    comptime if dtype.is_floating_point():
        return lane_kind(x)
    else:
        return 0


def forwarded[dtype: DType](x: Scalar[dtype]) -> Int where dtype.is_floating_point():
    return lane_kind(x) + 1


def signed_int[dtype: DType]() -> Int where dtype.is_integral() and not dtype.is_unsigned():
    return 1


def signed[dtype: DType]() -> Int where dtype.is_signed():
    return 2


def numeric[dtype: DType]() -> Int where dtype.is_numeric() and not dtype.is_half_float() and not dtype.is_float8():
    return 3


def flagged[b: Bool]() -> Int where b:
    return 5


def gated[b: Bool]() -> Int:
    comptime if b:
        return flagged[b]()
    return 0


struct Lanes[dtype: DType]:
    var v: Scalar[Self.dtype]

    def __init__(out self, v: Scalar[Self.dtype]):
        self.v = v

    def whole(self) -> Int where Self.dtype.is_integral():
        return Int(self.v)


def main():
    print(lane_kind(Float32(1.5)), guarded(Float64(2.0)), guarded(Int8(2)), forwarded(Float16(0.5)))
    print(signed_int[DType.int8](), signed_int[DType.int](), signed[DType.float16](), signed[DType.int64]())
    print(numeric[DType.uint8](), numeric[DType.float32]())
    print(Lanes(Int32(4)).whole(), flagged[True](), gated[True](), gated[False]())

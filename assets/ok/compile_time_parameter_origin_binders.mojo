# A compile-time parameter's origin binders solve from its bracket
# argument's type, as a run-time argument's solve from its place: a span
# over a compile-time list, forwarded from one such parameter to another,
# a slice of a local compile-time list (upstream's `_horner_evaluate`),
# and a list display, whose conversion borrows compile-time memory,
# immutable (upstream's `polynomial_evaluate[[c0, c1]](x)`).
def last[
    dtype: DType,
    width: SIMDLength,
    m: Bool,
    o: Origin[mut=m],
    //,
    c: Span[Scalar[dtype], o],
](x: SIMD[dtype, width]) -> SIMD[dtype, width]:
    return x + c[len(c) - 1]


def forwarded[
    dtype: DType,
    width: SIMDLength,
    m: Bool,
    o: Origin[mut=m],
    //,
    c: Span[Scalar[dtype], o],
](x: SIMD[dtype, width]) -> SIMD[dtype, width]:
    return last[c](x)


def inferred[c: Span[Int, _]](x: Int) -> Int:
    return x + len(c)


def sliced[dtype: DType, width: SIMDLength, //](x: SIMD[dtype, width]) -> SIMD[dtype, width]:
    comptime coefficients = List([Scalar[dtype](1.0), 2.0, 3.0])
    return last[coefficients[:] if dtype == .float64 else coefficients[:2]](x)


def displayed[dtype: DType, width: SIMDLength, //](x: SIMD[dtype, width]) -> SIMD[dtype, width]:
    return last[[Scalar[dtype](1.0), 2.0, 4.0]](x)


def immutable[m: Bool, o: Origin[mut=m], //, c: Span[Float64, o]]() -> Int:
    comptime assert not m
    return len(c)


comptime floats: List[Float64] = [1.0, 2.0, 3.0]
comptime more: List[Float64] = [4.0, 5.0]
comptime ints: List[Int] = [1, 2, 3]


def main():
    print(last[Span(floats)](Float64(1)), forwarded[Span(more)](Float64(1)))
    print(inferred[Span(ints)](1))
    print(sliced(Float64(1)), sliced(Float32(1)))
    print(displayed(Float64(1)), displayed(Float32(2)))
    print(inferred[[1, 2, 3, 4]](1), immutable[[1.0, 2.0]]())

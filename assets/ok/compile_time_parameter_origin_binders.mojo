# A compile-time parameter's origin binders solve from its bracket
# argument's type, as a run-time argument's solve from its place: a span
# over a compile-time list, forwarded from one such parameter to another,
# and a slice of a local compile-time list (upstream's `_horner_evaluate`).
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


comptime floats: List[Float64] = [1.0, 2.0, 3.0]
comptime more: List[Float64] = [4.0, 5.0]
comptime ints: List[Int] = [1, 2, 3]


def main():
    print(last[Span(floats)](Float64(1)), forwarded[Span(more)](Float64(1)))
    print(inferred[Span(ints)](1))
    print(sliced(Float64(1)), sliced(Float32(1)))

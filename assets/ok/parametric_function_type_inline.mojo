# A function type with its own `def[...]` list, spelled inline as a type
# argument, resolves its signature in the scope of that list.


struct S[F: AnyType]:
    pass


def main():
    comptime X = S[def[d: DType](Scalar[d]) thin -> Scalar[d]]
    comptime Y = S[def[d: DType](Scalar[d]) thin -> Scalar[d]]
    print(X == Y)
    comptime Z = S[def[T: Copyable](T) thin -> T]
    print(Z == Z)
    comptime N = S[S[def[d: DType, w: SIMDLength, //](SIMD[d, w]) thin -> SIMD[d, w]]]
    print(N == N)
    comptime W = S[def[w: Int](Int) thin -> None where w > 0]
    print(W == W)

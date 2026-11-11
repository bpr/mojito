# A struct conforming to a function type with its own value binders
# declares a `__call__` with the same binders, spelled as it likes, and a
# call through a bound of that type applies them: `f[2](40)` runs
# `__call__[2]`, and a binder the arguments determine is inferred.
struct Adder(def[n: Int](Int) -> Int):
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def __call__[k: Int](self, value: Int) -> Int:
        return self.base + k * value


struct Scaler[s: Int](def[n: Int](Int) -> Int):
    def __init__(out self):
        pass

    def __call__[n: Int](self, value: Int) -> Int:
        return Self.s * n + value


struct Summer(def[w: SIMDLength](SIMD[DType.int32, w]) -> Int):
    def __init__(out self):
        pass

    def __call__[w: SIMDLength](self, v: SIMD[DType.int32, w]) -> Int:
        return Int(v.reduce_add())


def invoke[F: def[n: Int](Int) -> Int](f: F) -> Int:
    return f[2](40)


def total[F: def[w: SIMDLength](SIMD[DType.int32, w]) -> Int](f: F) -> Int:
    return f(SIMD[DType.int32, 4](1, 2, 3, 4))


def main():
    print(invoke(Adder(3)))
    print(invoke(Scaler[3]()))
    print(Adder(5)[3](2))
    print(total(Summer()))

# A signature's `type_of` reads the parameters declared before it, as
# upstream's `Signatures.cpp` emits each type with the earlier arguments in
# scope: a later parameter's type, a result, and a method's result over
# `self`. A `_` in a `SIMD` parameter's type is an infer-only binder the
# argument solves, so `type_of(x)` is the vector type the call binds, as in
# the signatures of upstream's `std/math` `max` and `min`. A value
# parameter's `type_of` is its declared type.


def add(x: Int, y: type_of(x)) -> type_of(x):
    return x + y


def first(x: String, *rest: type_of(x)) -> type_of(x):
    return x + String(len(rest))


struct Counter:
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def next(self) -> type_of(self.v):
        return self.v + 1

    def scaled(self, by: type_of(self.v)) -> type_of(self):
        return Counter(self.v * by)


def simd_max[
    dtype: DType, //
](x: SIMD[dtype, _], y: type_of(x), /) -> type_of(x):
    comptime assert (
        dtype.is_numeric()
    ), "the SIMD type must be numeric"
    return x.gt(y).select(x, y)


def simd_min[
    dtype: DType, //
](x: SIMD[dtype, _], y: type_of(x), /) -> type_of(x):
    comptime assert (
        dtype.is_numeric()
    ), "the SIMD type must be numeric"
    return x.lt(y).select(x, y)


def twice[n: Int](extra: type_of(n)) -> type_of(n):
    return n * 2 + extra


def flipped[b: Bool]() -> type_of(b):
    return not b


def main():
    print(add(1, 2))
    print(first("a", "b", "c"))
    var c = Counter(3)
    print(c.next())
    print(c.scaled(5).v)
    print(simd_max(Float32(1.5), Float32(2.5)))
    print(simd_min(Float32(1.5), Float32(2.5)))
    var a = SIMD[DType.int32, 4](1, 5, 3, 7)
    var b = SIMD[DType.int32, 4](4, 2, 6, 0)
    print(simd_max(a, b))
    print(simd_min(a, b))
    print(twice[4](1), flipped[True]())

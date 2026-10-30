# A compile-time call applies a function over a type pack, as at the pin:
# `cnt(n, True)` binds `*Ts` from its collector's arguments, each taken at
# its own element (`cnt[Int, Bool](Int(5), True)`), and `many(n)` takes a
# pack its call binds to no type.
def cnt[*Ts: AnyType](*xs: *Ts) -> Int:
    return len(xs)


def many[*Ts: AnyType](n: Int) -> Int:
    return n


def counted[n: Int](
    x: SIMD[DType.int8, cnt(n, True)]
) -> SIMD[DType.int8, cnt(n, True)]:
    return x


def packed[n: Int]() -> SIMD[DType.int32, many(n)]:
    return SIMD[DType.int32, many(n)](7)


def main():
    print(counted[5](SIMD[DType.int8, cnt(5, True)](1)))
    print(packed[2]())

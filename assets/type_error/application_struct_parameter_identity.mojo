# An application over a struct argument is its own expression: the result
# `SIMD[DType.int8, mylen(Foo(n))]` is not `SIMD[DType.int8, n]`, as at the
# pin.
# expect: type mismatch for return
@fieldwise_init
struct Foo(ImplicitlyCopyable):
    var n: Int


def mylen(f: Foo) -> Int:
    return f.n


def g[n: Int]() -> SIMD[DType.int8, n]:
    return SIMD[DType.int8, mylen(Foo(n))](1)


def main():
    print(g[2]())

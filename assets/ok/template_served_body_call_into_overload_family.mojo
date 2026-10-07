# A template-served body's call over its own binder into an overload family
# that keeps a member the cloner specializes (`kind[T]`, whose body
# materializes a reflected list) is served by the declaration the checker
# selected, as at the pin: `inferred`, `explicit`, `by_value`, and `h` are
# served by their templates, while `g`, which selects the cloned member over
# its own binder, is cloned per call. A losing overload's probe of `n` as a
# type leaves nothing on the winning `kind[n]` call (`both`).
@fieldwise_init
struct P(Copyable):
    var a: Int
    var b: Int


def kind[T: AnyType](a: Int) -> Int:
    comptime names = reflect[T].field_names()
    var all = materialize[names]()
    return len(all) + a


def kind[dt: DType](a: Scalar[dt]) -> Int:
    return 3


def kind[n: Int](a: Float64) -> Int:
    return n


def inferred[dt: DType](a: Scalar[dt]) -> Int:
    return kind(a)


def explicit[dt: DType](a: Scalar[dt]) -> Int:
    return kind[dt](a)


def by_value[n: Int](a: Float64) -> Int:
    return kind[n](a)


def both[n: Int, T: AnyType](a: Float64) -> Int:
    return kind[T](1) + kind[n](a)


def g[T: AnyType](x: Int) -> Int:
    return kind[T](x)


def h[dt: DType](a: Scalar[dt]) -> Int:
    return kind[P](1) + kind(a)


def main():
    print(inferred(Int8(1)))
    print(explicit[DType.float32](2.0))
    print(by_value[5](1.0))
    print(kind[P](10))
    print(both[5, P](1.0))
    print(g[P](10))
    print(h(Int16(4)))

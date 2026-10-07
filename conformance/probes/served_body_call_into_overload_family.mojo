# Pin gap probe (Mojo 1.2.0.dev2026092105): a served generic body calling,
# over its own binder, into an overload family that holds a member the cloner
# specializes. The pin prints `3`, `3`, `5`, and `12`. Mojito rejects the
# inferred `kind(a)` with "generic 'kind' requires compile-time parameter
# 'T'", naming another overload's parameter, and the explicit `kind[n](a)`
# fails MIR verification, "value parameter 'n' has no runtime register".
# Roadmap R478.
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


def main():
    print(inferred(Int8(1)))
    print(explicit[DType.float32](2.0))
    print(by_value[5](1.0))
    print(kind[P](10))

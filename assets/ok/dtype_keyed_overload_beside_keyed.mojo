# A `DType`-keyed `def` overloaded with a compile-time-keyed one that the
# cloner specializes per call and a plain one. The checker selects the
# overload; a call selecting the `DType` member is served by its template, so
# only the keyed member is cloned.
# requires: discovery


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


def kind(a: String) -> Int:
    return 4


def main():
    print(kind[P](10))
    print(kind[DType.float64](1.8))
    print(kind(Int32(4)))
    var x: Float64 = 1.5
    print(kind(x))
    print(kind(String("s")))

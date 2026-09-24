# A static method's own compile-time parameters apply explicitly on every
# receiver spelling: a plain struct's bare name (`Lanes.ident[Int](3)`), the
# single-argument application that parses as a value subscript
# (`Box[String].ident[Int](3)`), and a type application (`Box[Int].ident[Int](3)`).
# The key may be a type or a `DType` value, the body may fold a `comptime if`
# on it, and the call may sit in a helper `def`; each call mints the same
# per-call clone the inferred spelling does.
# requires: discovery
struct Lanes:
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    @staticmethod
    def ident[T: ImplicitlyCopyable](x: T) -> T:
        return x

    @staticmethod
    def widen[dt: DType](a: Scalar[dt]) -> Scalar[dt]:
        return a * 3

    @staticmethod
    def show[T: Copyable](x: T):
        comptime if T == Int:
            print("int")
        else:
            print("other")


struct Box[U: Copyable & Movable & Deinitable]:
    var item: Self.U

    def __init__(out self, var item: Self.U):
        self.item = item^

    @staticmethod
    def ident[T: ImplicitlyCopyable](x: T) -> T:
        return x


def helper() -> Int:
    return Lanes.ident[Int](5)


def main():
    print(Lanes.ident[Int](3))
    print(Lanes.ident[Bool](True))
    print(Lanes.widen[DType.int32](3))
    print(Lanes.widen[DType.float64](1.5))
    Lanes.show[Int](3)
    Lanes.show[Float64](3.5)
    print(helper())
    print(Lanes.ident(4))
    var b = Box[String](String("hi"))
    print(b.item)
    print(Box[String].ident[Int](6))
    print(Box[Int].ident[Int](7))

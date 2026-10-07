# A value parameter typed by an earlier type parameter (`v: T`): the
# infer-only `T` is solved from the value, as an argument solves it.


@fieldwise_init
struct Pt(ImplicitlyCopyable, Writable):
    var x: Int

    def write_to(self, mut w: Some[Writer]):
        w.write("Pt(", self.x, ")")


struct S[T: ImplicitlyCopyable & Writable, //, v: T]:
    def __init__(out self):
        pass

    def get(self) -> Self.T:
        return Self.v


def f[T: ImplicitlyCopyable & Writable, //, v: T]() -> T:
    return v


def h[T: ImplicitlyCopyable & Deinitable & Writable, v: T]() -> T:
    var x: T = v
    return x


def name[T: ImplicitlyCopyable & Writable, //, v: T](s: S[v]) -> T:
    return v


def with_default[T: ImplicitlyCopyable & Writable, //, v: T = 3]() -> T:
    return v


def pair[T: ImplicitlyCopyable & Writable, //, v: T, w: T]() -> T:
    return w


struct Holder[T: ImplicitlyCopyable & Writable]:
    def __init__(out self):
        pass

    def pick[v: Self.T](self) -> Self.T:
        return v


def main():
    print(S[3]().get())
    print(f[True](), f[2.5](), f[Pt(4)]())
    var d: DType = f[DType.int8]()
    print(d)
    print(h[Int, 4](), h[Float64, 4]())
    var a = S[3]()
    var b = S[2.5]()
    var t = S[True]()
    print(name(a), name(b), name(t))
    var c: S[3] = a^
    print(name(c))
    print(with_default(), with_default[False]())
    print(pair[1, 2]())
    comptime x = f[3]()
    comptime y: Int = 5
    print(x, f[y](), f[1 + 2]())
    print(Holder[Int]().pick[5](), Holder[Float64]().pick[2]())

# A call of a local `comptime` type alias constructs the type the alias
# names: a closed `Scalar` or `SIMD` alias, one over a value binder or a
# local `comptime` value, a struct application, and an alias of a struct's
# type member that names a type parameter.


struct Sized[n: Int]:
    def __init__(out self):
        pass

    def size(self) -> Int:
        return Self.n

    def bump(self) -> Int:
        comptime Next = Sized[Self.n + 1]
        return Next().size()


struct Row[T: Copyable & Defaultable & Writable & Deinitable]:
    comptime First = Self.T

    @staticmethod
    def first() -> Self.T:
        comptime F = Self.First
        return F()


def scaled[dt: DType](x: Int) -> Scalar[dt]:
    comptime S = Scalar[dt]
    return S(x) * S(2)


def lanes[k: Int]():
    comptime N = k + 1
    comptime V = SIMD[DType.int32, N]
    print(V(7))


def sized[n: Int]() -> Int:
    comptime Next = Sized[n]
    return Next().size()


def main():
    comptime S = Scalar[DType.float64]
    print(S(2) * 1.25)
    comptime N = 2
    comptime V = SIMD[DType.int32, N]
    print(V(7))
    comptime I = Int
    print(I(3) + 4)
    comptime L = List[Int]
    var items = L()
    items.append(5)
    print(len(items))
    print(scaled[DType.int16](21))
    lanes[3]()
    print(sized[5]())
    print(Sized[4]().bump())
    print(Row[Int].first())

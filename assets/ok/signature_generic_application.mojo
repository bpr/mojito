# A call in a signature, a field type, or a type argument applies a callee
# generic over a type, with an explicit type argument, a defaulted binder,
# or a `DType` binder, as the pin's call does.
from std.sys.info import bit_width_of


struct Tag[k: Int]:
    def __init__(out self):
        pass

    def key(self) -> Int:
        return Self.k


def idt[T: ImplicitlyCopyable](x: T) -> T:
    return x


def first[T: ImplicitlyCopyable](a: T, b: T) -> T:
    return a


def size_of_type[T: AnyType]() -> Int:
    return 2


def defaulted[n: Int = 2]() -> Int:
    return n


def widened[dt: DType]() -> DType:
    return DType.int32 if dt == DType.float32 else dt


def splat[n: Int]() -> SIMD[DType.int32, idt(n)]:
    return SIMD[DType.int32, idt(n)](7)


def typed[n: Int]() -> SIMD[DType.int32, size_of_type[Int]()]:
    return SIMD[DType.int32, size_of_type[Int]()](n)


def counted[n: Int]() -> SIMD[DType.int32, defaulted()]:
    return SIMD[DType.int32, defaulted()](n)


def retyped[dtype: DType]() -> SIMD[widened[dtype](), 2]:
    return SIMD[widened[dtype](), 2](3)


def bytes[dt: DType]() -> SIMD[DType.int8, bit_width_of[dt]() // 8]:
    return SIMD[DType.int8, bit_width_of[dt]() // 8](1)


def tagged[n: Int](t: Tag[idt(n)]):
    print(n, t.key())


struct Box[n: Int]:
    var t: Tag[first(Self.n, 0)]

    def __init__(out self):
        self.t = Tag[first(Self.n, 0)]()


def main():
    print(splat[2]())
    print(typed[5]())
    print(counted[5]())
    print(retyped[DType.float32]())
    print(bytes[DType.int32]())
    tagged[1](Tag[idt(1)]())
    var b = Box[4]()
    print(b.t.key())

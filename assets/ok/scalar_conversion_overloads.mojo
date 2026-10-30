# `Int(x)`, `UInt(x)`, `Float64(x)`, and the sized aliases bind `SIMD`'s
# declared `Intable`, `IntableRaising`, `Floatable`, and `FloatableRaising`
# overloads, as at the pin: an `Intable` value converts through its
# `__int__` into any integral scalar (`Bool` and a pointer are `Intable`), a
# `Floatable` one through its `__float__`, and a raising dunder through the
# raising overload of a struct declaring it.
from std.memory.alloc import unsafe_alloc


struct F(Floatable, Intable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __int__(self) -> Int:
        return self.v

    def __float__(self) -> Float64:
        return Float64(self.v) + 0.5


struct C[n: Int](Intable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __int__(self) -> Int:
        return 7


struct Wide(Intable):
    def __init__(out self):
        pass

    def __int__(self) -> Int:
        return 300


struct R(FloatableRaising, IntableRaising):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __int__(self) raises -> Int:
        return self.v

    def __float__(self) raises -> Float64:
        return Float64(self.v)


def to_int[T: Intable](x: T) -> Int:
    return Int(x)


def to_float[T: Floatable](x: T) -> Float64:
    return Float64(x)


def to_uint[T: Intable](x: T) -> UInt:
    return UInt(x)


def lane[dt: DType](b: Bool) -> SIMD[dt, 1]:
    return SIMD[dt, 1](b)


def main() raises:
    print(Int(F(3)), Float64(F(3)), UInt(F(3)))
    print(to_int(F(5)), to_float(F(5)), to_uint(F(6)))
    print(Int64(C[4](1)), Int(C[4](1)), UInt(C[4](1)), UInt8(C[4](1)))
    print(Int8(Wide()))
    print(Int(R(3)), Float64(R(4)))
    print(Int("42"), Float64("2.5"))
    print(UInt8(True), SIMD[DType.int, 1](True), lane[DType.int8](True))
    var b = True
    print(Int(b), UInt(b), Float64(b), Int8(b))
    var f = -1.5
    print(Int(f), UInt(Int(-1)), Int8(Float64(3.9)))
    var p = unsafe_alloc[Int](2)
    print(Int(p) != 0, UInt(p) != 0, to_int(p) == p.__int__())
    p.unsafe_free()

# Explicit scalar construction converts runtime values: integers wrap to any
# integer width and convert to float lanes, floats adjust precision, and any
# Intable value (bounded parameter or conforming struct) constructs integer
# scalars through its __int__. A SIMD element narrows through its lane type:
# upstream's element argument is already the lane's scalar, and passing an
# out-of-range literal or a wider runtime value is the Mojito-only
# `simd-element-narrowing` conformance case.
@fieldwise_init
struct Meters(Intable):
    var count: Int

    def __int__(self) -> Int:
        return self.count

def to_byte[T: Intable](x: T) -> Byte:
    return Byte(x)

def main():
    var i = 300
    print(Byte(i))
    print(UInt32(UInt(70000)))
    print(Int32(Byte(200)))
    print(Float32(2.75))
    print(Float32(i))
    print(SIMD[DType.uint8, 4](Byte(i), 1, 2, Byte(259)))
    print(to_byte(Meters(261)))
    print(Byte(Meters(300)))

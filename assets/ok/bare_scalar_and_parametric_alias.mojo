# A bare `Scalar` is the prelude's partial alias `SIMD[_, length=1]`, and a
# bare module alias with compile-time parameters leaves each of them
# unbound, a defaulted one too, as a bare struct does: either one, spelled
# bare as an argument's type, is an infer-only parameter of the callee,
# solved per call.
comptime Lane[dt: DType] = SIMD[dt, 1]
comptime Wide[dt: DType = DType.int32] = SIMD[dt, 1]


def twice(x: Scalar) -> type_of(x):
    return x + x


def lane_twice(x: Lane) -> type_of(x):
    return x + x


def wide_twice(x: Wide) -> type_of(x):
    return x + x


def is_float(x: Scalar) -> Bool:
    return x.dtype.is_floating_point()


struct Scaler:
    var k: Int

    def __init__(out self, k: Int):
        self.k = k

    def scale(self, x: Scalar) -> type_of(x):
        return x * type_of(x)(self.k)


trait Doubler:
    def double(self, x: Scalar) -> type_of(x):
        ...


struct Adder(Doubler):
    def __init__(out self):
        pass

    def double(self, x: Scalar) -> type_of(x):
        return x + x


def main():
    print(twice(Int32(3)), twice(Float64(1.5)), twice(UInt8(100)))
    print(lane_twice(Int32(3)), lane_twice(Float64(1.5)))
    print(wide_twice(Float64(1.5)), wide_twice(Int32(3)))
    print(is_float(Int16(1)), is_float(Float32(1.0)))
    print(Scaler(3).scale(UInt16(7)), Scaler(2).scale(Float32(0.25)))
    print(Adder().double(Int64(5)))

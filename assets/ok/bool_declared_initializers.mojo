# `Bool(...)` binds upstream's declared initializers in `std.builtin.bool`:
# the default, `None`, the `Boolable` one over a user struct, a scalar, a
# literal, a string, and an `Optional`, the `@implicit` `Scalar[.bool]` one,
# and `Bool(value=i)` over an `i1`, whose body stores the `Bool`'s
# `_mlir_value`, at run time and in a `comptime` value.
from std.collections import Optional


struct Flag(Boolable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def __bool__(self) -> Bool:
        return self.x != 0


def truthy[T: Boolable](x: T) -> Bool:
    return Bool(x)


def main():
    print(Bool(), Bool(None))
    print(Bool(Flag(3)), Bool(Flag(0)))
    var t = Flag(2)
    print(Bool(value=t), truthy(t), truthy(Flag(0)))
    print(Bool(3), Bool(0), Bool(0.0), Bool(1.5))
    print(Bool(UInt(0)), Bool(Int64(-1)), Bool(Float32(0.5)))
    print(Bool(Scalar[DType.bool](True)), Bool("abc"), Bool(""))
    print(Bool(Optional[Int](3)), Bool(Optional[Int]()))
    var b = True
    print(Bool(b), Bool(copy=b), Bool(value=True.__mlir_i1__()))
    var s: Bool = Scalar[DType.bool](False)
    print(s)
    comptime c = Bool(3)
    comptime d = Bool(0.0)
    print(c, d, truthy(Float64(0)), truthy(True))

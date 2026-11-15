# A scalar or `SIMD` subject reflects `SIMD`'s one declared field,
# `_mlir_value`, directly, through a generic `def`, through a reflected
# field type, and as a `field_ref` read back through `Int(mlir_value=...)`.
@fieldwise_init
struct R(Copyable):
    var x: Int
    var y: Float64


def show[T: AnyType]() -> Int:
    return reflect[T].field_count()


def get0[T: AnyType](mut x: T):
    ref r = reflect[T].field_ref[0](x)


def width_one[d: DType]() -> Int:
    return reflect[SIMD[d, 1]].field_count()


def main():
    print(reflect[Int].field_count())
    print(reflect[Float64].field_count())
    print(reflect[Bool].field_count())
    print(reflect[UInt8].field_count())
    print(reflect[SIMD[DType.float32, 4]].field_count())
    print(reflect[Int].field_names()[0])
    print(reflect[Int].field_index["_mlir_value"]())
    print(show[Int](), show[Float64]())
    print(show[reflect[R].field["x"].T]())
    print(width_one[DType.int32]())
    var y = 4
    ref s = reflect[Int].field_ref[0](y)
    print(Int(mlir_value=s))
    var x = 3
    get0(x)
    print(x)

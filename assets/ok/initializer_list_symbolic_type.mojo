# An initializer list, or a construction or static call through
# `type_of`, at a type over parameters: the construction builds the
# contextual type itself, whose arguments have no closed spelling, and a
# `{}` at a reflected field type is its default.


struct It[T: AnyType](Movable):
    var i: Int

    def __init__(out self, i: Int):
        self.i = i


struct Holder[T: AnyType]:
    def __init__(out self):
        pass

    def it(self) -> It[Self.T]:
        return {0}


def make[T: AnyType]() -> It[T]:
    return {5}


def splat[dt: DType, w: SIMDLength](x: SIMD[dt, w]):
    var y: SIMD[dt, w] = {7}
    print(y)


def scalar_of[dt: DType](x: Scalar[dt]):
    print(type_of(x)(3))


def count[*Ts: Copyable](t: Tuple[*Ts]):
    print(type_of(t).__len__())


def type_list[*Ts: AnyType]():
    var x: TypeList[Trait=AnyType, Ts.values] = {}
    print(len(x))


struct Zero(Copyable, Defaultable, Writable):
    var v: Int

    def __init__(out self):
        self.v = 0

    def write_to(self, mut writer: Some[Writer]):
        writer.write("Zero")


@fieldwise_init
struct Fields(Copyable):
    var first: Int
    var second: Zero
    var third: Bool


def defaults[T: AnyType]():
    comptime types = reflect[T].field_types()
    comptime for i in range(reflect[T].field_count()):
        comptime if conforms_to(
            types[i], Defaultable & Deinitable & Writable & Copyable
        ):
            var x: types[i] = {}
            print("braced:", x)


def total(*values: Int) -> Int:
    var sum = 0
    for v in values:
        sum += v
    return sum


def pack_sum[*values: Int]() -> Int:
    var sum = 0
    for v in ParameterList[values.values]():
        sum += v
    return sum


def main():
    print(Holder[Int]().it().i)
    print(make[Bool]().i)
    splat(SIMD[DType.int16, 4](1))
    scalar_of(Float32(1))
    scalar_of(Int8(1))
    count((1, True, 2.5))
    var x: TypeList.of[Trait=AnyType, Int, Bool] = {}
    print(len(x))
    type_list[Int, Bool, Float64]()
    print(total(1, 2, 3))
    print(pack_sum[4, 5, 6]())
    defaults[Fields]()

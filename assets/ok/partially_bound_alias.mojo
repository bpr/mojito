# A `comptime` alias over a partially bound struct names a type once its
# open parameters are bound: a `_` leaves its own slot open, a trailing `...`
# every slot left, and an application binds the open slots positionally or
# by the struct's own parameter names. A struct member alias reads the
# same way through `Self` or a concrete instance, as does a struct pack
# element in a method's `comptime for`.
@fieldwise_init
struct Pair[a: Int, b: Int = 10](Writable):
    def write_to(self, mut writer: Some[Writer]):
        writer.write(Self.a, " ", Self.b)


comptime First = Pair[_]
comptime Both = Pair[...]
comptime Keyed = Pair[a=_, b=3]
comptime Second = Pair[1, _]
comptime Bytes = SIMD[.uint8, _]
comptime I32 = SIMD[DType.int32, _]


def double[w: SIMDLength](x: Bytes[w]) -> Bytes[w]:
    return x + x


struct Lanes[dtype: DType]:
    comptime _T = SIMD[Self.dtype, _]

    def __init__(out self):
        pass

    def splat[width: SIMDLength](self, v: Scalar[Self.dtype]) -> Self._T[width]:
        var lanes: Self._T[width] = v
        return lanes

    def show(self):
        var one: Self._T[1] = 1
        var two: Self._T[2] = 3
        var narrow: Lanes[DType.int8]._T[1] = 7
        print(one, two, narrow)
        print(self.splat[4](2))


struct Defaults[*Ts: Movable & Writable & Deinitable & Defaultable]:
    def __init__(out self):
        pass

    def show(self):
        comptime for i in range(len(Self.Ts)):
            var value: Self.Ts[i] = Self.Ts[i]()
            print(value)


def main():
    var first: First[5] = Pair[5]()
    var both: Both[1, 2] = Pair[1, 2]()
    var keyed_default: Both[a=6] = Pair[6]()
    var keyed: Keyed[7] = Pair[7, 3]()
    var second: Second[4] = Pair[1, 4]()
    print(first, both, keyed_default, keyed, second, sep=" | ")
    var named: I32[length=2] = 5
    print(named)
    print(double(SIMD[DType.uint8, 4](1, 2, 3, 4)))
    Lanes[DType.float32]().show()
    Defaults[Int, String, Bool]().show()

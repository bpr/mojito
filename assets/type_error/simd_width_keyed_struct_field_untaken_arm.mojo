# expect: type mismatch for variable 's': expected String, found SIMD[dt, width]
# A struct keyed on a dtype and a width holds a `SIMD[Self.dt, Self.width]`
# field with both slots symbolic; its method keyed on `Self.width` is
# validated once, so the arm the width-4 instance never selects is reported.
struct Buf[dt: DType, width: Int](Copyable, Movable):
    var v: SIMD[Self.dt, Self.width]

    def __init__(out self, v: SIMD[Self.dt, Self.width]):
        self.v = v

    def doubled(self) -> SIMD[Self.dt, 2 * Self.width]:
        return self.v.join(self.v)

    def tag(self) -> Int:
        comptime if Self.width == 4:
            return 4
        else:
            var s: String = self.v
            return 0


def main():
    var b = Buf[DType.int32, 4](SIMD[DType.int32, 4](1, 2, 3, 4))
    print(b.doubled(), b.tag())

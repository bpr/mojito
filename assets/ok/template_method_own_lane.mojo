# A method keyed on a `DType` or vector-width binder of its own derives each
# per-call clone from its checked template. A body constructing a vector at
# that lane (`Scalar[dt](x)`, `SIMD[DType.int32, w](x)`) is stubbed in the
# elaborated program, so source validation checks it with the binder
# symbolic; a body naming the lane only in its signature is checked as the
# template. Each clone folds the binder, a `DType` to a constant under the
# name's identity.
struct Box:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def lanes[w: Int](self) -> Int:
        var v = SIMD[DType.int32, w](self.x)
        return len(v) + Int(v.reduce_add())

    def kind[dt: DType](self, y: Int) -> Int:
        var one = Scalar[dt](y + self.x)
        return Int(one * one)

    def twice[dt: DType](self, a: Scalar[dt]) -> Scalar[dt]:
        return a + a

    @staticmethod
    def zero[dt: DType]() -> Scalar[dt]:
        return Scalar[dt](0)


struct Holder[T: Copyable & Movable & Deinitable]:
    var value: Self.T

    def __init__(out self, value: Self.T):
        self.value = value.copy()

    def make[dt: DType](self, x: Int) -> Scalar[dt]:
        return Scalar[dt](x)


def main():
    var b = Box(3)
    print(b.lanes[4](), b.lanes[2]())
    print(b.kind[DType.int16](2), b.kind[DType.uint8](1))
    print(b.twice(Int16(5)), b.twice(Float32(1.5)))
    print(Box.zero[DType.uint8](), Box.zero[DType.float64]())
    var h = Holder[Int](7)
    print(h.make[DType.int16](5), h.make[DType.float32](2))

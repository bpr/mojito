# A struct keyed on a `DType` binder, specialized whole, holds fields of its
# symbolic lane type (`Scalar[Self.dtype]`). Its members convert them
# (`Int(self.pos)`), construct one (`Scalar[Self.dtype](limit)`), combine
# them with each other and with literals, store and rebind them, and build
# another specialization at the same lane, and derive from their checked
# templates at every dtype, including `DType.int` and `DType.float64`, whose
# lanes are the native `Int` and `Float64`. A comparison between two lane
# values keeps the clone check: it is a mask on a vector lane but a `Bool`
# on a native one.
struct Walker[dtype: DType](Copyable, ImplicitlyCopyable, Movable):
    var pos: Scalar[Self.dtype]
    var step: Scalar[Self.dtype]

    def __init__(out self, pos: Scalar[Self.dtype], step: Scalar[Self.dtype]):
        var first = pos
        if Int(first) < 0:
            first = 0
        self.pos = first
        self.step = step

    def advance(mut self):
        self.pos += self.step
        self.pos += 1

    def back(self) -> Scalar[Self.dtype]:
        return 2 * self.pos - self.step

    def at(self, idx: Int) -> Scalar[Self.dtype]:
        return self.pos + Scalar[Self.dtype](idx) * self.step

    def flipped(self) -> Walker[Self.dtype]:
        return Walker[Self.dtype](-self.pos, Scalar[Self.dtype](-1) * self.step)

    def distance(self) -> Int:
        var total = self.pos + self.step
        if Int(total) > 100:
            total = 100
        return Int(total) - Int(self.pos)

    def below(self, limit: Int) -> Bool:
        if self.pos < Scalar[Self.dtype](limit):
            return True
        return False


def show[dt: DType](start: Int):
    var w = Walker[dt](Scalar[dt](start), Scalar[dt](2))
    w.advance()
    var f = w.flipped()
    print(w.pos, w.back(), w.at(3), f.pos, f.step, w.distance(), w.below(10))


def main():
    show[DType.int](1)
    show[DType.int16](2)
    show[DType.int64](-3)
    show[DType.float32](4)
    show[DType.float64](5)

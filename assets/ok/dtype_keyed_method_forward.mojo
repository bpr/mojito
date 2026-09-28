# A method keyed on its own [dt: DType] may forward that lane to a
# DType-keyed def (`helper[dt](x)`), alone or beside another key
# (`pair[dt, 4](x)`), from an instance or a static method: each call's clone
# reaches the def's clone for its own lane.
# requires: discovery
def helper[dt: DType](x: Int) -> Scalar[dt]:
    return Scalar[dt](x)


def pair[dt: DType, n: Int](x: Int) -> SIMD[dt, n]:
    return SIMD[dt, n](x)


struct Maker:
    def __init__(out self):
        pass

    def make[dt: DType](self, x: Int) -> Scalar[dt]:
        return helper[dt](x)

    def scaled[dt: DType](self, x: Int) -> Scalar[dt]:
        var v = helper[dt](x)
        return v * 2

    def vec[dt: DType](self, x: Int) -> SIMD[dt, 4]:
        return pair[dt, 4](x)

    @staticmethod
    def stat[dt: DType](x: Int) -> Scalar[dt]:
        return helper[dt](x) + helper[dt](1)


def main():
    var m = Maker()
    print(m.make[DType.int32](7))
    print(m.make[DType.float64](3))
    print(m.scaled[DType.int16](5))
    print(m.vec[DType.float32](2))
    print(Maker.stat[DType.uint8](9))

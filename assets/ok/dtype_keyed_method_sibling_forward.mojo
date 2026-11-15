# A method keyed on its own [dt: DType] may forward that lane to a sibling
# DType-keyed method (`self.twice[dt](a)`, `self.twice[dt=dt](a)`,
# `Self.st[dt](a)`), in a plain or a parametric struct: the sibling binds the
# caller's own `dt`, so both sides name the same `Scalar[dt]`.
struct S:
    def __init__(out self):
        pass

    def twice[dt: DType](self, a: Scalar[dt]) -> Scalar[dt]:
        return a + a

    def via[dt: DType](self, a: Scalar[dt]) -> Scalar[dt]:
        return self.twice[dt](a)

    def via_var[dt: DType](self, a: Scalar[dt]) -> Scalar[dt]:
        var r = self.twice[dt](a)
        return r

    def via_kw[dt: DType](self, a: Scalar[dt]) -> Scalar[dt]:
        return self.twice[dt=dt](a)

    @staticmethod
    def st[dt: DType](a: Scalar[dt]) -> Scalar[dt]:
        return a * 3

    def via_static[dt: DType](self, a: Scalar[dt]) -> Scalar[dt]:
        return Self.st[dt](a)


@fieldwise_init
struct G[dtype: DType]:
    def tw[dt: DType, n: Int](self, a: SIMD[dt, n]) -> SIMD[dt, n]:
        return a + a

    def via[dt: DType, n: Int](self, a: SIMD[dt, n]) -> SIMD[dt, n]:
        return self.tw[dt, n](a)


def main():
    var s = S()
    print(s.via[DType.int64](7))
    print(s.via[DType.float32](7))
    print(s.via_var[DType.int8](7))
    print(s.via_kw[DType.uint16](7))
    print(s.via_static[DType.int32](7))
    var g = G[DType.int8]()
    print(g.via[DType.int16, 2](7))

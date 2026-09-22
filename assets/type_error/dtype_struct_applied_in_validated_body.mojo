# expect: type mismatch for variable 's': expected String, found Float64
# A validated body that applies a `DType`-keyed struct reaches a registered
# declaration, not a template shell that ends the run without a verdict: the
# instance's `Scalar[Self.dt]` field closes to `Float64`, and the invalid
# untaken arm is reported.
struct Vec[dt: DType]:
    var x: Scalar[Self.dt]

    def __init__(out self, x: Scalar[Self.dt]):
        self.x = x


def build[T: Copyable](t: T) -> Float64:
    comptime if T == Int:
        var v = Vec[DType.float64](2.0)
        return v.x
    else:
        var v = Vec[DType.float64](2.0)
        var s: String = v.x
        return 0.0


def main():
    print(build(1))

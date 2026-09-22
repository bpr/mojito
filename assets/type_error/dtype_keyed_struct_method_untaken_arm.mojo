# expect: type mismatch for variable 's': expected String, found Scalar[dt]
# A `DType`-keyed struct is no longer a template shell: its method keyed on
# `Self.dt` is validated with the field's `Scalar[Self.dt]` symbolic, so the
# arm the float64 instance never selects is still reported.
struct Vec[dt: DType]:
    var x: Scalar[Self.dt]
    var y: Scalar[Self.dt]

    def __init__(out self, x: Scalar[Self.dt], y: Scalar[Self.dt]):
        self.x = x
        self.y = y

    def sum(self) -> Scalar[Self.dt]:
        return self.x + self.y

    def tag(self) -> Int:
        comptime if Self.dt == DType.float64:
            return 1
        else:
            var s: String = self.x
            return 2


def main():
    var v = Vec[DType.float64](1.5, 2.5)
    print(v.sum(), v.tag())

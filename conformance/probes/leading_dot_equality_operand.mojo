# PROBE: a leading-dot operand of `==`, and a leading-dot case head over a subject that is not `EnumLike`, take the selected `__eq__` parameter's type as their context.
#
# The pin runs it and prints:
#   True
#   i64
#   x
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: rejects `d == .int64` ("no contextual type is available here")
# until R1238 lands; `case .X` also needs R489.
#
# Run:    mojo run leading_dot_equality_operand.mojo
#         cargo run -- run conformance/probes/leading_dot_equality_operand.mojo
struct K(Equatable, ImplicitlyCopyable):
    var v: Int
    comptime X = K(1)

    def __init__(out self, v: Int):
        self.v = v

    def __eq__(self, other: Self) -> Bool:
        return self.v == other.v


def main():
    var d = DType.int64
    print(d == .int64)
    match d:
        case .int64:
            print("i64")
        case _:
            print("other")
    var k = K(1)
    match k:
        case .X:
            print("x")
        case _:
            print("other")

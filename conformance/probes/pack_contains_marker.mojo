# PROBE (re-probe): `comptime if Self.Ts.contains[T]()` with a symbolic `T`
# checks both arms.
#
# The pin rejects the untaken `else` arm from the template: "cannot implicitly
# convert 'StringLiteral[\"no\"]' value to 'Int'". Membership is not decided
# symbolically; it is a residual condition. Mojito rejects likewise: "type
# mismatch for return: expected Int, found StringLiteral".
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`.
#
# Run:    mojo run pack_contains_marker.mojo
#         cargo run -- run conformance/probes/pack_contains_marker.mojo
struct Bag[*Ts: Movable](
    Deinitable where Ts.all_conforms_to[Deinitable](),
    Movable,
):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple[*Self.Ts](*args^)

    def has[T: AnyType](self) -> Int:
        comptime if Self.Ts.contains[T]():
            return 1
        else:
            return "no"


def main():
    print(1)

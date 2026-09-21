# PROBE (re-probe): a constant index into a symbolic pack keeps the dependent
# element.
#
# The pin rejects: "cannot implicitly convert 'Ts.values[SIMDLength(Int(0))]'
# value to 'Int'". `self.storage[0]` is accepted as an expression; its type is
# the opaque element at index 0, never a concrete type. Mojito rejects too,
# one rule earlier: "cannot copy non-Copyable type 'Ts[0]'", since the pack's
# bound is only `Movable`.
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`.
#
# Run:    mojo run pack_constant_index_symbolic_pack.mojo
#         cargo run -- run conformance/probes/pack_constant_index_symbolic_pack.mojo
struct Bag[*Ts: Movable](
    Deinitable where Ts.all_conforms_to[Deinitable](),
    Movable,
):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple[*Self.Ts](*args^)

    def first(self) -> Int:
        var x: Int = self.storage[0]
        return x


def main():
    print(1)

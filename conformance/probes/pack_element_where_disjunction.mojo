# PROBE (re-probe): a disjunctive `where` licenses no element use.
#
# The pin rejects: "an element of 'values' with type 'Ts.values[...]' does not
# conform to trait 'Writable'". Only a conjunctive assumption refines the
# element. Mojito rejects likewise, with the same wording.
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`.
#
# Run:    mojo run pack_element_where_disjunction.mojo
#         cargo run -- run conformance/probes/pack_element_where_disjunction.mojo
struct Bag[*Ts: Movable](
    Deinitable where Ts.all_conforms_to[Deinitable](),
    Movable,
):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple[*Self.Ts](*args^)

    def show(self) where Self.Ts.all_conforms_to[Writable]() or Self.Ts.all_conforms_to[Hashable]():
        comptime for i in range(Self.Ts.length):
            print(self.storage[i])


def main():
    print(1)

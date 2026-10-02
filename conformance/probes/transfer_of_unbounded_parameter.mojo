# PROBE (divergence): a `^` transfer of a value whose parameter type has no
# `Movable` bound.
#
# The pin rejects the declaration, whatever instances exist: "cannot transfer
# value into destination, because 'T' doesn't conform to 'Movable'". Mojito
# checks the transfer per instance, so this program, whose only instance is
# movable, runs and prints 3. Filed in `docs/roadmap.md` §3 ("A `^` transfer
# of an unbounded parameter type is accepted"). When Mojito rejects it,
# promote this file to `assets/type_error/` with the pin's diagnostic.
#
# Run:    mojo run transfer_of_unbounded_parameter.mojo
#         cargo run -- run conformance/probes/transfer_of_unbounded_parameter.mojo
@fieldwise_init
struct Holder[T: Deinitable]:
    var uses: Int

    def forward(mut self, var item: Self.T) -> Self.T:
        self.uses += 1
        return item^


def main():
    var h = Holder[Int](0)
    print(h.forward(3))

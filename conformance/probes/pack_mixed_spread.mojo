# PROBE (re-probe): a pack spread beside a fixed argument in a variadic
# application.
#
# The pin rejects the declaration: "invalid unpack in non-variadic parameter
# binding". A mixed list is a verdict, not a shape to model. Mojito rejects
# it from the template too: "a variadic pack spread must be the only argument
# it binds".
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`.
#
# Run:    mojo run pack_mixed_spread.mojo
#         cargo run -- run conformance/probes/pack_mixed_spread.mojo
struct Lead[*Ts: Movable & Deinitable](Movable):
    var storage: Tuple[Int, *Self.Ts]

    def head(self) -> Int:
        return self.storage[0]


def main():
    print(1)

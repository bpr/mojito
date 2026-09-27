# PROBE: a trailing `where` clause on a trait method.
#
# Mojito prints 1; the pin rejects the declaration with "'where' clauses on
# trait methods are not supported". Mojito accepts a program the pin rejects,
# so this is a divergence.
#
# Observed 2026-09-26 against `Mojo 1.6.0.dev2026092105`.
#
# Run:    mojo run where_on_trait_method.mojo
#         cargo run -- run conformance/probes/where_on_trait_method.mojo
trait Sink:
    def push[H: Movable](mut self, value: H) where conforms_to(H, Copyable): ...


def main():
    print(1)

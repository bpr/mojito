# PROBE: each iteration of an unrolled `comptime for` body is its own scope.
#
# The pin prints `0` then `1`. Mojito reports "'v' is already declared in
# this scope": the elaborator splices every unrolled copy of the body into
# the enclosing block, so a `var` declared in the body is redeclared on the
# second iteration. `docs/roadmap.md` §3 carries it; delete this probe when
# the entry closes and promote the program to `assets/ok/`.
#
# Observed 2026-09-23 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run comptime_for_body_scope.mojo
#         cargo run -- run conformance/probes/comptime_for_body_scope.mojo
def show[T: AnyType]():
    comptime for i in range(2):
        var v: Int = i
        print(v)


def main():
    show[Int]()

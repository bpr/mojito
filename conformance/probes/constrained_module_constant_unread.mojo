# PROBE: an unread module constant constrained by a `where` clause whose
# operand applies a function.
#
# The pin prints 1: a non-generic constrained constant is a generator type,
# checked only where it is used. Mojito stops with "unsupported generic
# constraint operand" though nothing reads it.
#
# Observed 2026-10-08 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run constrained_module_constant_unread.mojo
#         cargo run -- run conformance/probes/constrained_module_constant_unread.mojo
def f(x: Int) -> Int:
    return x + 1

comptime C: Int where (f(1) > 5, "big only") = f(1)

def main():
    print(1)

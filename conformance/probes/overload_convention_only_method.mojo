# PROBE (divergence): methods cannot overload on the parameter convention.
#
# The pin accepts a read and an owned (`var`) method of the same parameter
# type, and ranks the pair as it ranks the free-function one: the place costs
# the `var` candidate the copy it makes, and the rvalue selects it. Mojito
# rejects the declaration — `same_method_shape` (`checker/traits.rs`) compares
# parameter types and ignores conventions, and the lowered method name carries
# no owned-parameter qualifier where a free function's does.
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`:
#   pin:    2 then 1
#   mojito: rejected, "'m' is already declared in this scope"
#
# `assets/ok/overloading_owned_parameter.mojo` is the free-function pair that
# both compilers accept.
#
# Run:    mojo run overload_convention_only_method.mojo
#         cargo run -- run conformance/probes/overload_convention_only_method.mojo
#
# When fixed: this file moves to `assets/ok/`.
struct S:
    var v: Int

    def __init__(out self):
        self.v = 0

    def m(self, var a: String) -> Int:
        return 1

    def m(self, a: String) -> Int:
        return 2


def main():
    var s = String("s")
    var x = S()
    print(x.m(s))
    print(x.m(String("t")))

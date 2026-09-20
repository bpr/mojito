# PROBE (divergence): an implicit copy into a `var` parameter costs at the pin.
#
# A place passed to `h(var a: String)` must be copied, and the pin ranks that
# copy below a conversion but above the generic tie-break, so it selects the
# generic overload. Mojito does not count the copy and selects the `var` one:
# a silent wrong answer. An rvalue needs no copy and both select the `var` one.
#
# Observed 2026-09-19 against `Mojo 1.1.0.dev2026082605 (dd957314)`:
#   pin:    2 then 1
#   mojito: 1 then 1
# `h(var a: String, b: Int)` beside `h(a: String, b: Float64)` with `h(s, 1)`
# prints 1 at both, so the copy ranks below one conversion.
#
# Run:    mojo run overload_var_copy_beside_generic.mojo
#         cargo run -- run conformance/probes/overload_var_copy_beside_generic.mojo
#
# When fixed: `overload_rank` (`checker/overload_support.rs`) gains a copy term
# between the conversion count and the variadic bit, `VariadicBinding` drops
# its own, and this file moves to `assets/ok/`.
def h(var a: String) -> Int:
    return 1


def h[T: Writable](a: T) -> Int:
    return 2


def main():
    var s = String("s")
    print(h(s))
    print(h(String("t")))

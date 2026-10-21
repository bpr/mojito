# PROBE: a nested `def` or lambda that captures a reference binding by `imm` or `var` copies its referent.
#
# The pin runs it and prints:
#   331
#
# Mojito: "copied value has type Int, destination has type ref Int" for the
# `{var a}` def; alone, the `{imm a}` def fails elaboration ("loads the handle
# of reference slot ... instead of its referent"), and the `{var r}` lambda
# runs on the VM but crashes natively. Each capture keeps the binding's `ref`
# type instead of its referent's (R1230).
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
#
# Run:    mojo run reference_binding_value_capture.mojo
#         cargo run -- run conformance/probes/reference_binding_value_capture.mojo
def main():
    var xs: List[Int] = [1, 2]
    var total = 0
    for a in xs:
        def f() {imm a} -> Int:
            return a * 10
        def h() {var a} -> Int:
            return a * 100
        total = total + f() + h()
    ref r = xs[0]
    total = total + (lambda {var r} -> Int: r)()
    print(total)

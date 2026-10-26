# PROBE: a nested `def` or lambda that captures a reference binding by `imm` or `var` copies its referent.
#
# The pin runs it and prints:
#   331
#
# Mojito prints 331 on the VM but crashes natively: the two `def`s copy or
# borrow the referent through their closure's storage, while the `{var r}`
# lambda still captures the `ref` itself as a leading argument (R1427).
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

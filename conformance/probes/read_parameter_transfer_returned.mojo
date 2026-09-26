# PROBE (divergence): a `^` transfer of a read parameter, returned.
#
# A parameter with no `var` convention is read-only, so `x^` cannot be
# transferred out of it. Mojito accepts the transfer, and the callee's drop
# then frees the caller's `String`: the run fails with "use after Pointer
# deallocation". The pin's diagnostic is still to be observed. Filed in
# `docs/roadmap.md` §3 ("A `^` transfer of a read parameter is accepted and
# frees the caller's value"). When Mojito rejects it, promote this file to
# `assets/type_error/` with the pin's diagnostic.
#
# Run:    mojo run read_parameter_transfer_returned.mojo
#         cargo run -- run conformance/probes/read_parameter_transfer_returned.mojo
def ident(x: String) -> String:
    return x^


def main():
    var s = String("s")
    print(ident(s))
    print(s)

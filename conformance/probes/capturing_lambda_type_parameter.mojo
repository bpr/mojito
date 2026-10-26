# PROBE: a capturing lambda bound to a compile-time callable parameter.
#
# The pin rejects it:
#   error: cannot use a capturing lambda in type parameter
#
# Mojito: "compiler invariant violated: lambda reached inference before
# scoped checking" (R1231). When Mojito rejects it with the pin's
# diagnostic, move this file to `assets/type_error/` with its
# `conformance/assets-mojo-errors.tsv` row.
#
# Observed 2026-10-25 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
#
# Run:    mojo run capturing_lambda_type_parameter.mojo
#         cargo run -- run conformance/probes/capturing_lambda_type_parameter.mojo
def observe[origins: OriginSet, //, f: def(x: Int) capturing[origins] -> Int](value: Int) -> Int:
    return f(value)

def main():
    var factor = 3
    print(observe[lambda (x: Int) -> Int: x * factor](5))
